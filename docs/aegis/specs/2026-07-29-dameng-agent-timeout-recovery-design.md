# Dameng Agent 超时隔离与自动恢复设计

## 1. 文档状态

- 状态：已批准并实施；客户端缺陷与架构退役门禁均已验证
- 日期：2026-07-29
- 首个落地驱动：Dameng / DM8
- 共享能力范围：Java JDBC Agent 公共层、Agent JSON-RPC 协议、Rust 连接运行时和桌面端状态收敛
- 架构评审：必需
- 实施状态：方案 C 主恢复链已落地并通过真实 DM8 验证；见本文末“实施审计”

本设计扩展 `docs/pips/PIP-0001-database-connection-timeout-recovery.md`，沿用其中 `DbOperationBudget`、SQL 执行超时与基础设施硬超时分离、超时后淘汰故障资源、前端状态必须收敛等原则。本设计不替代 PIP-0001，而是补足 JDBC Agent，尤其是 Dameng 驱动拒绝或延迟响应取消时的强隔离能力。

## 2. 摘要

当前故障不是“缺少 HikariCP”这一个问题。真实 DM8 验证和代码检查共同表明，故障由以下因素叠加产生：Dameng Agent 长期持有单一物理连接、Session 数据操作使用粗粒度同步锁、取消依赖 JDBC 驱动协作、大量元数据查询绕过统一执行器，以及重连仍依赖旧 Session 正常退出。

本设计采用方案 C：

1. Rust 侧将元数据操作和用户工作负载拆分为独立 lane。
2. Java Agent 将控制面和数据面分离，控制请求不等待 SQL 执行锁。
3. 所有 JDBC 操作统一进入 `JdbcExecutor`，每个操作都有 `operationId`、截止时间和 Statement 登记。
4. 连接由可替换的 `ConnectionProvider` 提供；Dameng 默认使用 Hikari 实现，单连接实现仅保留为兼容和测试载体。
5. 超时后依次执行协作式取消、有限宽限、连接隔离和异步中止；不能确认安全的连接永不复用。
6. 使用单调递增的 session generation 隔离旧请求；旧结果晚到必须丢弃。
7. 重连先创建并验证新 generation，再原子替换；旧资源清理不得阻止恢复。
8. UI 在失败时保留最后一次成功的对象树，不得把错误转换为“没有找到对象”。

HikariCP 是连接创建、验证和正常复用的实现，不是最终故障恢复边界。最终恢复边界是可以被整体替换的 Agent session generation；即使 JDBC 驱动完全忽略 timeout、cancel、close 和 abort，DBX 仍必须在固定上限内停止等待，并在新 generation 上恢复服务。

## 3. 已证实事实

### 3.1 真实 DM8 验证基线

验证使用 DM DBMS `8.1.3.12` 和 `DmJdbcDriver8 8.1.5.45`。测试凭据和地址不写入仓库、测试源码、日志或本文档。

| 场景 | 实测结果 | 设计结论 |
| --- | --- | --- |
| 基础连接和 `SELECT 1` | 正常 | JDBC URL 和基础连通性不是根因 |
| 复杂纯 `SELECT`，`setQueryTimeout(2)` | 约 4.24 秒返回 `-608 / 执行超时` | 驱动超时可能延迟，外层仍需硬上限 |
| 复杂纯 `SELECT`，显式 `Statement.cancel()` | cancel 约 167 ms 返回；查询约 1.65 秒返回 `-6515 / 操作被取消` | 协作式取消有价值，应作为第一阶段 |
| 上述取消后复用同一连接 | `SELECT 1` 正常 | 仅在明确确认操作结束且连接验证成功时允许复用 |
| `CALL SP_SLEEP(30)`，`setQueryTimeout(2)` | 无效，实际约 30.1 秒 | 不得把 JDBC query timeout 当作硬恢复边界 |
| `CALL SP_SLEEP(30)`，`Statement.cancel()` | cancel 返回，但执行线程 5 秒后仍未退出 | cancel 返回不等于 SQL 已停止 |
| 随后调用 `Connection.abort()` | socket 被关闭，Statement 清理仍抛通信异常 | 中止后连接必须隔离，清理错误不得污染新连接 |
| 裸 JDBC 空闲 70 秒后 `isValid` | 正常，约 28 ms | 本次环境未复现服务端 idle 断链 |
| 空闲 70 秒后对象查询 | 正常返回 909 个对象，约 86 ms | “空闲即失败”不是已证实的 DM8 服务端行为 |

端到端 Agent 复现：同一 `agentSessionId` 执行 `CALL SP_SLEEP(10)`，`timeoutSecs=2`，500 ms 后发送 `cancel_session`，随后发送 `list_objects` 和 `close_session`。`cancel_session` 约 421 ms 返回 `ok`，SQL 仍完整运行约 10.04 秒；对象刷新和关闭均阻塞超过 3 秒。SQL 结束后刷新与关闭发生竞态，刷新最终返回 `Not connected`。

因此，用户观察到的“超时后所有查询超时”“刷新显示没有找到对象”“重连无效”与客户端队头阻塞和状态竞态一致，不能归因于 DM8 空闲后必然断开。

### 3.2 当前代码事实

- `DamengAgent` 以字段保存一个长期物理 `Connection`。
- `MultiSessionJsonRpcServer.Session.handle()` 和 `close()` 使用同一对象级同步锁；执行中的 SQL 未返回时，验证、刷新、关闭和重连相关数据请求排队。
- `Session.cancel()` 不获取该锁，但只调用 `JdbcExecutor.cancelActiveStatements()`，无法强制驱动停止。
- `JdbcExecutor` 能登记其自身创建的 Statement，但 Dameng 元数据路径存在多处直接 `prepareStatement().executeQuery()`，不受统一超时和取消控制。
- Rust 在 Agent RPC 超时后会发送 `cancel_session`，并在部分错误路径移除 pool；但共享客户端互斥和 Java Session 锁使这些恢复动作仍可能等待旧操作。
- `close_session` 当前需要进入 Session 的同步关闭路径，不能作为可靠的带外恢复边界。

## 4. 问题定义

### 4.1 根因

根因是 DBX 把“单次 JDBC 调用能够及时返回”当作连接生命周期管理的隐含前提，又把同一 Session 的执行、刷新、验证、关闭和重连绑定到同一个串行执行边界。当驱动不响应取消时，一个旧操作同时占据物理连接、Session 锁和客户端等待链，导致后续所有动作发生队头阻塞。

连接池缺失会放大问题，但单纯引入连接池不会移除 Session 粗锁、补齐未登记 Statement、提供带外控制通道或隔离旧结果，因此不是充分修复。

### 4.2 客户端级“完美解决”定义

DBX 无法承诺数据库服务端中的任意 SQL 或存储过程都一定被终止；该能力受 DM JDBC 和数据库内核控制。DBX 必须无条件保证以下客户端行为：

1. 任意单个 JDBC 调用不能永久阻塞控制面、对象树刷新或后续数据库操作。
2. 所有等待、取消、隔离和清理阶段都有明确硬上限。
3. 状态不明、超时或被中止的物理连接绝不再次承载业务请求。
4. 新查询和对象刷新能在新 generation 上恢复，不需要重启 DBX。
5. 旧 generation 的迟到结果、错误和事件不能修改新 generation 的状态。
6. 元数据 lane 与工作负载 lane 相互隔离；一方阻塞不能造成另一方队头阻塞。
7. 事务、游标、分页、临时表、当前 Schema 和其他连接级状态保持正确的连接亲和性。
8. UI 必须退出执行中、取消中和刷新中状态，并准确区分超时、取消已确认、连接已隔离和重连失败。
9. 重复超时不会导致连接、Session、线程、游标或任务数量无界增长。

## 5. 目标与非目标

### 5.1 目标

- 首先完整修复 Dameng Agent 的超时、取消、刷新和重连恢复链路。
- 将协议和公共 Java 抽象设计为其他 JDBC Agent 可选择复用的能力。
- 统一 Dameng 的用户 SQL、元数据、DDL 辅助查询、Explain、分页、导出和事务执行入口。
- 保持现有连接配置中 `query_timeout_secs = 0` 表示“不限制 SQL 本身执行时间”的语义。
- 建立可量化、可故障注入、可在真实 DM8 环境复验的验收标准。
- 通过 SOLID、明确所有权和受控依赖方向保证实现可维护性。

### 5.2 非目标

- 不保证 DBX 能终止所有已经进入 DM8 服务端执行的过程。
- 不在首期同时迁移所有 JDBC Agent 到 Hikari。
- 不改变 DM8 服务端参数、会话限制、资源管理器或 DBA 权限配置。
- 不以后台无限重试掩盖错误，也不自动重放可能产生副作用的 SQL。
- 不改变用户选择 `query_timeout_secs = 0` 时的无限 SQL 执行语义；用户主动取消、断开或应用退出仍可触发隔离。
- 不允许通过共享全局静态连接池跨越不同连接配置、用户身份或数据库实例。

## 6. 第一性原理与设计不变量

### 6.1 第一性原理审查

**不可协商目标：**任何单个 JDBC 调用都不能永久阻断 DBX 的控制面和后续数据库操作。

**不可协商约束：**事务、分页游标、临时表、Schema 和 Session 状态必须保持连接亲和性；状态不明的连接不能复用；旧 generation 结果不能进入新状态。

**应删除的历史假设：**`Statement.cancel()` 一定有效；`Connection.close()` 一定及时；一个 Agent Session 只需要一把同步锁；增加连接池就等于具备故障恢复。

**最小充分路径：**保留 JDBC 协作式取消作为快速路径，同时增加 operation 登记、控制/数据面分离、连接隔离、session generation 替换和双 lane。

**升级信号：**若同一 JVM 内无法隔离失控 Session，或失控线程/连接超过资源阈值，则升级为替换 Agent 进程 generation，而不是继续等待旧 JVM。

### 6.2 所有权与退休矩阵

| 关注点 | 新权威所有者 | 旧所有者或问题路径 | 退休条件 |
| --- | --- | --- | --- |
| operation 生命周期和 generation | Rust `AgentSessionRuntime` 与 Java `SessionRuntime` 按协议共同校验，Rust 决定当前 generation | RPC 调用方的零散 timeout 分支 | v3 operation/generation 覆盖所有 Dameng RPC 后删除字符串推断式恢复分支 |
| JDBC Statement 执行和登记 | `JdbcExecutor` | `DamengAgent` 内直接 JDBC 执行 | 所有 Dameng JDBC 路径迁移且静态检查无直连执行后删除旧路径 |
| 物理连接租约 | `ConnectionProvider` / `ConnectionLease` | `DamengAgent.connection` 字段 | 分页、事务和普通执行全部使用租约后删除字段 |
| 连接故障判定 | `ConnectionLease.invalidate()` 与 session quarantine 状态机 | 普通 `close()`、错误字符串间接清池 | 故障注入验证连接绝不回池后退休旧判断 |
| 当前 UI 数据快照 | generation-aware store reducer | 刷新开始即清空、错误映射为空列表 | UI 竞态测试覆盖后删除旧清空逻辑 |
| Session 串行语义 | 细粒度 affinity scope | `Session.handle()` 对所有方法加同步锁 | 控制面带外可达且亲和操作有专用锁后删除粗锁 |

`SingleConnectionProvider` 仅是兼容和测试实现，不得成为 Dameng 默认恢复路径。Dameng 默认使用 `HikariConnectionProvider`；两者必须满足同一租约契约和替换测试。

### 6.3 反证场景

- 驱动同时忽略 `setQueryTimeout`、`Statement.cancel()` 和 `Connection.abort()`：客户端仍必须在硬上限内切换到新 generation。
- 旧查询在新 generation 生效后返回成功：结果必须被丢弃且只记录审计日志。
- metadata lane 连接失效：workload lane 中已开始的事务不得被关闭。
- workload lane 中存储过程永久阻塞：metadata lane 仍能刷新；新 workload generation 能执行 `SELECT 1`。
- 新 generation 建立失败：保留最后成功对象树并显示重连错误，不能谎报空库，也不能把旧 generation 恢复为当前状态。

## 7. 候选方案与决策

| 方案 | 优点 | 缺点 | 决策 |
| --- | --- | --- | --- |
| A. 只增加 HikariCP | 改动较小；改善正常连接复用和验证 | Session 粗锁仍阻塞；未登记 Statement 仍不可取消；不能隔离迟到结果 | 拒绝作为完整修复 |
| B. 每个请求独立 Agent Session/物理连接 | 请求隔离最强，模型直观 | 连接数和建连成本高；事务、游标、临时表和 Session 状态难以保持 | 拒绝作为通用客户端模型 |
| C. 控制/数据面分离、双 lane、租约和 generation | 固定时间恢复；保持会话语义；资源和迟到结果可治理 | 跨 Rust、协议、Java 和 UI，实施复杂度较高 | 采用 |

## 8. 目标架构

```text
Desktop UI / Store
        |
        v
Rust Connection Runtime
  +-- MetadataLaneRuntime ---- agentSessionId M / generation m
  +-- WorkloadLaneRuntime ---- agentSessionId W / generation w
          |
          v
Concurrent Agent RPC Transport
  +-- request writer：仅写入期间持锁
  +-- response router：按 JSON-RPC id 分发，不跨 await 持有客户端互斥锁
          |
          v
Java MultiSession Runtime
  +-- Control Plane：cancel_operation / quarantine_session / close_session
  +-- Data Plane：metadata / query / page / transaction / DDL
          |
          v
SessionRuntime
  +-- OperationRegistry
  +-- AffinityCoordinator
  +-- JdbcExecutor
  +-- ConnectionProvider
        +-- SingleConnectionProvider（兼容、测试）
        +-- HikariConnectionProvider（Dameng 默认）
                |
                v
          HikariDataSource -> DM JDBC physical connections
```

### 8.1 Lane 边界

每个 DBX 逻辑连接拥有两个独立 lane：

- `METADATA`：对象树、Schema、表、列、索引、约束、触发器、对象源码和补全元数据。
- `WORKLOAD`：用户 SQL、Explain、分页读取、导出、批处理和事务。

两个 lane 使用不同的 `agentSessionId`、generation、`SessionRuntime` 和 `ConnectionProvider`。不得只在同一个 Session 内用两个线程模拟 lane，否则同一个失效 Provider 或连接级状态仍会产生连带故障。

metadata lane 的重建不得影响 workload lane；workload lane 的超时默认不清理 metadata lane。仅当共享 Agent JVM 被判定为不可用或协议传输损坏时，才同时替换两条 lane 所在的 process generation。

### 8.2 控制面与数据面

控制方法必须由 `MultiSessionJsonRpcServer` 在进入任何数据面互斥区之前路由。以下操作属于控制面：

- `cancel_operation`
- `quarantine_session`
- `close_session`
- `session_status`
- `shutdown`

控制面只能读取并更新线程安全的 registry/state，不得等待执行线程结束、JDBC `close()` 完成或事务锁释放。返回 `accepted` 表示控制动作已被登记，不表示服务端 SQL 已停止。

数据面不再使用覆盖整个 `dispatchForRuntime()` 的 `synchronized`。需要连接亲和性的操作由 `AffinityCoordinator` 对具体 affinity key 串行化；无亲和性的 metadata 和普通查询可按 Provider 容量并发。锁不得覆盖 RPC 输出、异步清理或新 generation 建立。

控制面使用独立的有界执行器，保留至少一个不被数据任务占用的工作线程；数据面执行器容量与 Provider 最大租约数对齐并使用有界队列。不得继续使用无上限的 cached thread pool。队列满时返回结构化 `AGENT_OVERLOADED`，不得无限创建线程或阻塞 stdin reader。JDBC abort/close 清理使用第三个有界执行器；清理线程耗尽时直接升级为 process generation 替换。

### 8.3 并发 RPC 传输

Rust 的 Agent 客户端必须支持多个并发 in-flight 请求：

- 发送侧只在写入一条完整 JSON-RPC 消息时持锁。
- 等待响应时不持有全局客户端互斥锁。
- 单一 reader 按响应 `id` 将结果投递到对应 waiter。
- 请求 timeout 后移除 waiter；迟到响应只能被记录并丢弃。
- 进程退出或 stdout 协议损坏时，一次性失败所有 waiter，并触发 process generation 替换。

这项改造是控制面真正“带外可达”的前提。仅移除 Java `synchronized` 而保留 Rust 跨 await 互斥，不能通过验收。

## 9. SOLID 与整洁代码约束

### 9.1 单一职责原则

- `AgentSessionRuntime`：管理 lane、generation、原子替换和恢复；不解释 JDBC 错误细节。
- `SessionRuntime`：管理 Java Session 状态和 operation 生命周期；不创建数据库特定 SQL。
- `JdbcExecutor`：执行、登记和关闭 JDBC 资源；不决定 UI 状态或 Agent 进程替换。
- `ConnectionProvider`：供应连接租约；不处理 JSON-RPC。
- `ConnectionLease`：管理一次借用及其有效性；不拥有连接配置解析。
- `DamengMetadataRepository`：只定义 Dameng 元数据 SQL和结果映射；执行委托给 `JdbcExecutor`。
- UI store：按 generation 收敛可见状态；不推断底层连接是否可复用。

### 9.2 开闭原则

- 增加新的连接供应策略时，通过实现公共 Provider 契约扩展，不修改 `JdbcExecutor` 的分支树。
- 增加新的 JDBC Agent 时复用 operation、deadline 和 lease 抽象；数据库特有 SQL留在驱动模块。
- 超时策略通过不可变配置对象注入，不在方法中散布 Dameng 特判常量。
- 禁止以不断扩张的 `if (dbType == ...)` 承载 Provider 行为。

### 9.3 里氏替换原则

`SingleConnectionProvider` 与 `HikariConnectionProvider` 必须满足相同语义：

- 成功返回的 lease 在释放前独占其连接使用权。
- `release()` 只归还状态明确且通过策略要求验证的连接。
- `invalidate(cause)` 幂等，调用后连接永不再被获取。
- Provider 关闭和 lease 失效都有上限，不能要求调用方无限等待。
- 获取超时、Provider 已关闭和连接创建失败使用同一结构化错误分类。

公共契约测试必须对两个实现运行同一测试套件。任何只对 Hikari 成立、却被公共接口承诺的行为都视为 LSP 违规。

### 9.4 接口隔离原则

避免建立一个同时包含查询、取消、池管理和 Session 管理的“大接口”。最小接口按使用者拆分：

```text
ConnectionProvider.acquire(context) -> ConnectionLease
ConnectionLease.connection()
ConnectionLease.release()
ConnectionLease.invalidate(cause)

StatementRegistry.register(operationId, statement) -> Registration
StatementRegistry.cancel(operationId) -> CancelAttempt

SessionControl.cancel(operationId)
SessionControl.quarantine(reason)
SessionControl.status()
```

业务查询只依赖获取 lease 和执行 SQL所需的接口；控制面只依赖 registry 和 session control；测试替身不需要实现无关方法。

### 9.5 依赖倒置原则

- `JdbcExecutor` 依赖 `ConnectionProvider`、`StatementRegistry`、`DeadlinePolicy` 抽象，不直接依赖 `HikariDataSource`。
- Hikari 适配器位于基础设施层，并通过构造注入进入 Dameng Session。
- `DamengAgent` 依赖公共执行端口，不直接调用 `DriverManager` 或持有长期 Connection。
- Rust 连接管理依赖 Agent capability/协议契约，不依赖 Dameng Java 类。
- 时间、UUID、后台执行器和故障清理调度均使用可注入端口，以支持确定性测试。

### 9.6 代码可读性与注释规则

- 类和方法名称表达领域动作，避免 `Manager`、`Helper`、`Utils` 承载多重职责。
- 状态转换集中实现，不允许多个模块直接修改 operation 或 generation 状态。
- 优先使用短方法、不可变值对象、结构化错误和 `try-with-resources`。
- 注释用于解释“为什么必须这样做”、驱动反常行为和并发不变量；不注释显而易见的赋值或控制流。
- DM8 特殊行为旁必须引用对应测试名称或错误码，不在注释中写环境地址和凭据。
- 禁止吞掉取消、隔离和清理异常；允许降级为日志，但必须带 operation/session/generation 上下文。

## 10. Java 公共组件契约

### 10.1 `OperationContext`

每个 JDBC 调用创建不可变上下文：

| 字段 | 含义 |
| --- | --- |
| `operationId` | 全局唯一，由 Rust 创建并透传 |
| `agentSessionId` | 当前 lane 的 Session 标识 |
| `generation` | 当前 Session 代际 |
| `lane` | `METADATA` 或 `WORKLOAD` |
| `operationType` | query、metadata、transaction、page、explain、ddl 等 |
| `queryTimeoutSecs` | 用户 SQL超时；0 表示无限 |
| `executionDeadline` | 本次执行的客户端截止时间；用户 SQL可为空，metadata 必须有限 |
| `infrastructureDeadline` | checkout、控制和清理的硬截止时间 |
| `affinityKey` | 可空；事务、分页或 Session 状态的亲和键 |

Java 必须在执行前检查 Session generation 与状态；不匹配时不触碰 JDBC，直接返回 `STALE_GENERATION`。

### 10.2 `ConnectionProvider` 与 `ConnectionLease`

Provider 的职责仅为：有限时获取租约、报告容量、停止接受新租约、异步关闭。租约是 JDBC Connection 的唯一合法持有方式。

租约状态：

```text
ACQUIRED -> RELEASED
ACQUIRED -> INVALIDATED -> DISPOSING -> DISPOSED
```

- `RELEASED` 的安全连接可由 Provider 复用。
- `INVALIDATED` 后必须先从可分配集合移除，再异步调用 abort/close。
- `invalidate` 与 `release` 竞态时，失效优先，连接不得回池。
- 销毁失败只影响旧资源清理，不得把连接恢复为可用。
- 重复调用必须幂等并留下可观测结果。

### 10.3 `HikariConnectionProvider`

Dameng 每个 Session generation 拥有独立 `HikariDataSource`，禁止跨 generation 复用 DataSource。这确保旧池中仍被失控线程占用的连接不会占据新 generation 的容量。

首期默认配置：

| 配置 | metadata lane | workload lane | 说明 |
| --- | --- | --- | --- |
| `minimumIdle` | 0 | 0 | 桌面客户端避免空闲时固定占用过多会话 |
| `maximumPoolSize` | 2 | 2 | 允许有限并发和验证；每个逻辑连接总上限通常为 4 |
| `connectionTimeout` | 来自 `checkout_timeout`，1–300 秒 | 同左 | 不受 query timeout 为 0 影响 |
| `validationTimeout` | 不超过 5 秒且不大于 connection timeout | 同左 | 连接借出前或恢复验证的上限 |
| `idleTimeout` | 默认 10 分钟，可由连接空闲配置覆盖 | 同左 | 不作为故障恢复手段 |
| `maxLifetime` | 默认 30 分钟，可配置 | 同左 | 加随机抖动，避免同时换代 |
| `keepaliveTime` | 与应用 keepalive 策略协调，默认 30 秒 | 同左 | 预防手段，不替代硬超时 |
| `autoCommit` | 驱动默认，借出后由执行上下文显式设置 | 同左 | 归还前必须复位或销毁 |

Hikari 的具体取值在实施计划中需校验当前依赖版本的合法下限。若 Hikari 自身最小值与本表冲突，使用最接近且不放宽 DBX 外层硬截止时间的值。

不能把 Hikari 的 `softEvictConnections()` 当作强制恢复完成条件。隔离顺序必须是：先让旧 Provider 对新请求不可见，再创建新 generation，最后在有界后台清理中调用 Hikari eviction、`Connection.abort(executor)` 和 `close()`。旧连接未释放不占用新 DataSource 的容量。

### 10.4 连接亲和性

以下操作必须绑定同一 workload lease：

- 显式事务从 begin 到 commit/rollback。
- 分页查询或表数据读取从创建游标到关闭游标。
- 依赖临时表、会话变量或会话级配置的操作序列。
- `SET SCHEMA` 与依赖该 Schema 的 SQL及复位过程。

亲和 lease 不允许被其他 operation 并发使用。亲和 operation 超时或失去状态确认后，整个 affinity scope 和 lease 一并失效；禁止在新连接上自动续接旧事务、游标或临时表。

普通无状态 metadata 或查询可以按次获取和归还 lease。执行结束前必须完成 JDBC 资源关闭和必要的 Connection 状态复位；任一复位失败则 invalidate。

### 10.5 `JdbcExecutor` 统一入口

所有 Dameng JDBC 调用必须经统一模板执行：

1. 校验 operation、generation、Session 状态和 deadline。
2. 从 affinity scope 或 Provider 获取 lease。
3. 创建 Statement 后立即登记到 `StatementRegistry`。
4. 应用 query timeout、fetch size、max rows 等策略。
5. 执行并读取结果。
6. 在 `finally` 中注销 Statement，关闭 ResultSet/Statement，复位连接状态。
7. 根据结果 release 或 invalidate lease。
8. 仅当 operation 仍属于当前 generation 且状态允许时返回结果。

元数据查询使用参数化的 `PreparedStatement` 执行端口和行映射器。`DamengAgent` / `DamengMetadataRepository` 可以拥有 SQL文本和映射规则，但不得直接调用 `prepareStatement()`、`executeQuery()` 或操作 Connection。

构建和测试增加架构约束：Dameng 驱动模块除 Provider 工厂/适配器外，不得出现 `DriverManager.getConnection`；Repository 不得出现 `.execute*(`；允许列表必须精确到基础设施文件，不能按整个目录豁免。

## 11. 状态机

### 11.1 Operation 状态

```text
CREATED -> QUEUED -> RUNNING -> SUCCEEDED
                         +----> FAILED
                         +----> CANCEL_REQUESTED -> CANCELLED_CONFIRMED
                                              +--> ABANDONED
RUNNING ------------------------------ timeout --> CANCEL_REQUESTED
```

- `CANCELLED_CONFIRMED`：执行线程已退出，Statement 已注销，租约状态已确定。
- `ABANDONED`：调用方已停止等待，但底层线程可能仍运行。进入此状态必须 invalidate lease 并 quarantine 当前 Session generation。
- 终态不可逆；迟到结果不能把 `ABANDONED` 改为 `SUCCEEDED`。
- 同一个 `operationId` 的取消必须幂等。

### 11.2 Session generation 状态

```text
OPENING -> ACTIVE -> SUSPECT -> QUARANTINED -> RETIRING -> RETIRED
    +------> FAILED

ACTIVE -- graceful close --> RETIRING
```

- `SUSPECT`：已触发取消，短宽限期内不接收新的数据面请求。
- `QUARANTINED`：立即从可路由集合移除；控制面仍可查询状态；Provider 不再发放租约。
- `RETIRING`：执行有界异步清理。
- `RETIRED`：registry 和资源引用已释放；不要求不可控的驱动线程已经自然返回。

Rust 对每个 lane 保存唯一的 `currentGeneration`。只有 `ACTIVE` 且 generation 匹配的响应能进入上层。

### 11.3 Agent process generation

若出现以下任一情况，替换整个 Agent 进程 generation：

- JSON-RPC reader/writer 损坏或进程退出。
- 新 Session 无法在连接硬超时内建立，但独立 TCP 探测表明数据库可达，且旧 JVM 存在失控 operation。
- 失控线程或 quarantined Session 达到资源阈值。
- 控制面在硬上限内无响应。

新进程 ready 和 handshake 成功后才能创建新 lane。旧进程关闭采用有限等待；超限后由 Rust 终止其子进程。进程替换只针对 DBX 启动并拥有的 Agent 子进程，不操作数据库服务进程。

## 12. 核心流程

### 12.1 正常执行

1. Rust 为请求生成 `operationId`，附加 lane 和当前 generation。
2. Java Session 校验 generation，登记 operation。
3. `JdbcExecutor` 获取租约、登记 Statement 并执行。
4. 资源关闭且租约安全后返回结果。
5. Rust 再次核对 lane 当前 generation，接受结果。
6. UI 只应用 generation 和 request token 均匹配的更新。

### 12.2 SQL 超时或用户取消

默认控制预算沿用 PIP-0001，并细化为：

| 阶段 | 默认上限 | 结果 |
| --- | --- | --- |
| JDBC query timeout | 用户 `query_timeout_secs`；0 表示无限 | 驱动协作式超时 |
| Rust 执行兜底 | query timeout + 2 秒；query timeout 为 0 时不自动触发 | 防止 Agent 丢失驱动超时响应 |
| metadata 执行 | `metadataLoadTimeoutMs`；query timeout 为 0 时仍使用 60 秒 | 对象刷新属于客户端基础设施，不允许无限等待 |
| `cancel_operation` 接受确认 | 1 秒 | 超限即认为控制面异常 |
| cancel 宽限 | 2 秒 | 等待执行线程确认退出 |
| session quarantine | 500 ms 内完成内存状态切换 | 先隔离，后清理 |
| 新 generation 建立 | `connect_timeout_secs + 2 秒`，且不低于 5 秒 | 验证后原子替换 |
| 旧 Session 清理 | 3 秒 | 超限后停止等待 |
| Agent 子进程优雅退出 | 3 秒 | 超限后终止子进程 |

用户主动取消时从 `cancel_operation` 开始；query timeout 为 0 不禁止用户取消。任何阶段的实现可以更快返回，但不得放宽硬上限。

流程：

1. 将 operation 标记为 `CANCEL_REQUESTED`，发送 `Statement.cancel()`。
2. 若执行线程在 2 秒内退出并完成资源判定，返回 `CANCELLED_CONFIRMED`。
3. 否则标记 operation 为 `ABANDONED`、租约为 `INVALIDATED`、Session 为 `QUARANTINED`。
4. 立即启动新 generation，不等待旧 `abort/close`。
5. 旧 Provider 在后台执行 Hikari eviction、`Connection.abort()` 和关闭。
6. 新 generation 验证成功后原子替换，并允许新请求进入。
7. 旧 operation 的任何结果被 response router 和 store 双重丢弃。

对只读 SQL也不自动重放，因为不能可靠识别存储函数、副作用、锁和会话状态。用户可在新 generation 上明确重新执行。

### 12.3 对象树刷新

- 刷新固定发送到 metadata lane，不与 workload lane 共用 Session 或 Provider。
- 刷新开始时保留当前快照，仅设置局部 loading 状态。
- 成功且 generation/request token 匹配时原子替换快照。
- 超时或失败时保留旧快照，展示“对象加载失败”及可重试动作。
- 只有成功响应且对象集合确实为空时显示“没有找到对象”。
- metadata lane 被隔离时自动建立新 generation 并最多重试一次仅包含只读 metadata 的请求；该重试使用新的 `operationId` 并记录 `retryOf`。
- 自动重试仍失败时停止，不循环重试。

### 12.4 手动重新连接

手动重连是“替换”，不是“先同步关闭再打开”：

1. 对两个 lane 分别创建 `generation + 1`。
2. 建立新 Session 并执行连接验证。
3. 两个新 lane 均成功后，以一个 Rust 原子状态更新替换当前连接视图。
4. 旧 lane 立即 quarantine，并异步清理。
5. 任一新 lane 失败则不发布半连接状态；保留旧 UI 快照并返回明确错误。已经不可用的旧 lane 仍保持隔离，不能回滚为 ACTIVE。

进行中的显式事务不会迁移；手动重连前必须提示并终止其客户端句柄。不得在新连接上伪造事务延续。

## 13. Agent 协议 v3

### 13.1 能力协商

新增协议版本 3 和能力：

- `operation_control`
- `session_generation`
- `lane_isolation`
- `structured_agent_errors`

握手仍返回 `protocolVersion`、`agentProtocolVersion` 和 `capabilities`。Rust 仅在四项能力齐全时启用强恢复路径。Dameng 发布物必须完整支持 v3；缺少任一能力时，连接测试返回“当前 Dameng Agent 不支持安全超时恢复”，不得静默降级到存在已知缺陷的 v2 路径。

其他 JDBC Agent 可继续使用 v2；本次不强制迁移，也不错误宣称其具备 v3 恢复保证。

### 13.2 请求字段

所有 v3 数据面请求必须包含：

```json
{
  "agentSessionId": "...",
  "generation": 3,
  "operationId": "...",
  "lane": "WORKLOAD"
}
```

控制方法：

| 方法 | 必填字段 | 返回语义 |
| --- | --- | --- |
| `cancel_operation` | session、generation、operationId | `accepted`、当前 operation 状态；不承诺服务端已停止 |
| `quarantine_session` | session、generation、reason | 状态切换完成即返回，不等待 JDBC 清理 |
| `session_status` | session、generation | Session 状态、活动 operation 数和隔离原因 |
| `close_session` | session、generation | 从路由表移除并登记清理后返回 |

v2 `cancel_session` 保留给旧 Agent；v3 调用方不得用它代替精确的 `cancel_operation`。分页返回的 `sessionId` 继续表示 cursor session，但必须同时绑定父 `operationId` 和 generation。

### 13.3 结构化错误

JSON-RPC error `data` 至少包含：

| 字段 | 说明 |
| --- | --- |
| `category` | `QUERY_TIMEOUT`、`CANCELLED`、`CONNECTION_BROKEN`、`SESSION_QUARANTINED`、`STALE_GENERATION`、`CHECKOUT_TIMEOUT`、`AGENT_OVERLOADED`、`AGENT_UNAVAILABLE`、`DATABASE_ERROR` |
| `retryable` | 是否允许客户端对安全操作发起新 operation |
| `connectionDisposition` | `REUSABLE`、`INVALIDATE_LEASE`、`QUARANTINE_SESSION`、`REPLACE_PROCESS` |
| `operationId` | 关联 operation |
| `agentSessionId` | 关联 Session |
| `generation` | 关联代际 |
| `driverCode` | 可空，例如 DM `-608`、`-6515` |

Rust 和 UI 按 category/disposition 处理，不再依赖英文错误字符串包含 `timed out` 或 `Not connected`。

## 14. UI 行为与状态收敛

- 查询 timeout：显示“查询已超时，客户端已停止等待；正在隔离原连接”。
- cancel 已确认：显示“查询已取消”。
- cancel 未确认但已隔离：显示“已停止等待并隔离原连接，数据库服务端操作可能仍在结束中”。
- 自动恢复成功：连接状态恢复为可用，不自动重跑用户 SQL。
- 自动恢复失败：显示明确重连错误和重试入口，不要求重启软件。
- 对象树刷新失败：保留最后成功数据和展开状态；错误提示与空结果提示互斥。
- 每次异步 store action 捕获开始时的 generation 和 request token；结束时不匹配则无副作用退出。
- `isExecuting`、`isCancelling`、metadata loading 在各自前端硬上限内必须进入终态，即使 Tauri/Rust Promise 没有正常完成。

## 15. 可观测性

日志使用结构化字段，不记录密码、完整 JDBC URL、敏感 SQL参数或结果数据：

- `connectionId`、`agentProcessGeneration`
- `agentSessionId` 的不可逆短哈希、`sessionGeneration`、`lane`
- `operationId`、`operationType`、`affinityKey` 的不可逆短哈希
- `phase`：checkout、execute、cancel、grace、quarantine、abort、reconnect、cleanup
- `elapsedMs`、`deadlineMs`
- `outcome`、`errorCategory`、`driverCode`、`connectionDisposition`
- 当前 active/quarantined Session、operation、lease 和 Agent 线程计数

必须提供以下指标或等价诊断快照：

- operation 各终态计数与耗时分布。
- cancel accepted、cancel confirmed、abandoned 数量。
- lease invalidation 和 session/process generation replacement 数量。
- 丢弃迟到响应数量。
- 每个逻辑连接的物理连接数、quarantined Session 数和失控线程数。

日志应能回答一次故障卡在 checkout、JDBC execute、cancel、cleanup、RPC 传输还是 UI 收敛阶段。

## 16. 资源与安全边界

- 默认每个逻辑 Dameng 连接最多两个 active lane generation，每个 generation 仅有一个当前版本。
- 正常情况下物理连接上限为 metadata 2 + workload 2，共 4；generation 切换期间允许短暂达到 8。
- 每个逻辑连接最多保留 2 个 quarantined Session；超过阈值触发 Agent process generation 替换。
- 同一 Agent 进程失控 operation 达到 4 个或失控线程达到 4 个时替换进程；阈值应可测试但不暴露为普通用户配置。
- 旧 Session 和进程引用在清理上限后从当前运行时释放，诊断记录保留有限环形缓冲，不允许内存无界增长。
- Agent 子进程终止必须校验 PID 属于当前 DBX 启动并持有的 child handle；禁止按名称批量终止 Java 进程。
- 自动重试只允许 metadata、连接验证等明确幂等操作，最多一次。用户 SQL、DDL、批处理和事务不自动重试。
- 真实环境凭据只从用户连接配置进入运行时，不进入测试夹具、Spec、日志或异常消息。

## 17. 迁移与旧路径退休

迁移必须按可独立验证的阶段进行，但最终发布不得同时保留两个 Dameng 执行所有者：

1. 定义 v3 协议契约、结构化错误和 Rust 并发 response router。
2. 在 Java 公共层引入 operation/session 状态机、最小接口和契约测试。
3. 实现 `SingleConnectionProvider` 作为测试基准，再实现 Hikari 适配器并运行同一 Provider 契约测试。
4. 建立 Dameng metadata/workload 双 lane 和 generation 替换。
5. 将 Dameng 所有 JDBC 路径迁移到 `JdbcExecutor`，包括元数据直连路径。
6. 增加 UI generation guard、错误展示和对象树快照保留。
7. 完成单元、集成、故障注入和真实 DM8 验收。
8. 删除 `DamengAgent.connection`、Dameng Repository 的直接 JDBC 执行、Session 级粗锁和 Dameng v2 静默降级。

在第 8 步完成前，只能作为开发分支过渡状态，不能宣称缺陷修复完成。架构检查必须验证：

- Dameng 业务层没有直接 Connection 所有权。
- 所有 Statement 都能映射到 operation。
- 控制面请求无须获得数据面锁。
- 新 generation 的建立不依赖旧 generation 的 close 完成。
- 连接池、Session 和 UI 各自只有一个权威状态所有者。

## 18. 测试策略

### 18.1 单元和契约测试

- 对 `SingleConnectionProvider`、`HikariConnectionProvider` 运行相同 acquire/release/invalidate/close 契约测试。
- 验证 invalidate 与 release 竞态始终以失效为准。
- Operation 每个合法/非法状态转换及幂等 cancel。
- generation 比较和迟到响应丢弃。
- RPC writer 并发写完整性、response id 路由和 waiter timeout 清理。
- 结构化错误到 Rust disposition、UI 文案和状态转换的映射。
- affinity scope 对事务、分页和 Schema 操作的连接固定。
- 静态架构测试阻止 Dameng Repository 直接执行 JDBC。

### 18.2 Java/Rust 集成故障注入

使用可控假驱动分别模拟：

- query timeout 生效。
- cancel 立即生效、延迟生效和永不生效。
- abort 生效、抛错和永久阻塞。
- Connection close 阻塞。
- checkout、validation 和连接创建超时。
- 旧 operation 在新 generation 发布后成功或失败返回。
- metadata lane 和 workload lane 分别永久阻塞。
- RPC 控制响应丢失、乱序和 Agent 进程退出。

所有永久阻塞模拟必须使用测试可终止的 latch，测试结束后断言无线程和资源泄漏。

### 18.3 真实 DM8 回归矩阵

使用外部注入的测试连接配置，至少覆盖：

| 场景 | 预期 |
| --- | --- |
| `SELECT 1`、对象列表 | 正常，结果正确 |
| 复杂纯 `SELECT` + 2 秒 timeout | UI 退出执行；连接按实际结束状态复用或隔离 |
| 复杂纯 `SELECT` + 用户取消 | 5 秒内终态；随后查询正常 |
| `CALL SP_SLEEP(30)` + 2 秒 timeout | 不等待 30 秒；旧 workload generation 隔离，新 generation 可查询 |
| `CALL SP_SLEEP(30)` + 用户取消 | cancel 不生效时仍切换 generation |
| workload 长调用期间刷新对象树 | metadata 在其预算内正常返回，不等待 workload |
| metadata 故障期间执行用户查询 | workload 不受影响 |
| 超时后立即手动重连 | 不等待旧 close，新 generation 验证后可用 |
| 空闲超过 70 秒后刷新 | 成功或明确触发重建；不显示伪空列表 |
| 旧结果延迟返回 | UI 和缓存无变化，迟到计数增加 |
| 显式事务中超时 | 事务 scope 失效，不在新连接续接 |
| 分页游标中超时 | 游标关闭或废弃，新 generation 不接受旧 cursor id |
| 连续 20 次不可取消调用 | 客户端持续可操作，连接/线程/Session 不无界增长，必要时替换 Agent 进程 |

测试结束必须查询并记录 DBX 可见的 Agent 资源计数；在有权限时辅助检查 DM8 会话，但“服务端会话立即消失”不是客户端验收前提。

## 19. 量化验收标准

以下条件必须全部满足，才能宣称该缺陷已解决：

1. `CALL SP_SLEEP(30)` 配置 2 秒 timeout 时，查询 UI 在 5 秒内退出执行中状态。
2. cancel 请求 1 秒内被控制面接受；执行未在额外 2 秒内退出时，Session 在 500 ms 内完成内存隔离。
3. 数据库可达时，新 workload generation 在 `connect_timeout_secs + 2 秒` 内通过 `SELECT 1` 验证，不等待原 30 秒调用。
4. workload lane 阻塞期间，对象树刷新不进入同一等待队列，并在 metadata 预算内完成。
5. metadata lane 阻塞期间，新的普通 workload 查询可执行。
6. 任何被 invalidate 的物理连接在测试中被再次借出的次数为 0。
7. 新 generation 生效后，旧 generation 的成功、错误、空结果和进度事件对 UI/store 的写入次数为 0。
8. 对象刷新失败时，最后成功对象快照保持不变；“没有找到对象”只由成功空响应触发。
9. 手动重连不等待旧 Session close；数据库可达时无需重启 DBX 即可恢复。
10. 连续 20 次故障注入后，当前 lane 各只有一个 active generation；quarantined Session 不超过 2；超阈值时完成 Agent process 替换。
11. 两个 Provider 实现通过同一公共契约测试，Dameng 所有 JDBC 执行通过统一 executor 的架构检查。
12. `query_timeout_secs = 0` 时，不因本设计自动终止正常长 SQL；用户主动取消和基础设施硬超时仍有效。
13. 日志中能通过 operation/session/generation 关联完整恢复路径，且不包含测试密码、完整连接串或敏感参数。

## 20. 回滚策略

- v3 通过 capability 协商启用，不改变其他 v2 Agent 行为。
- 若 Hikari 适配器出现回归，可在开发/诊断构建切换到 `SingleConnectionProvider` 验证问题边界；Dameng 正式发布不得以此开关关闭强恢复保证。
- UI generation guard、结构化错误处理和旧结果丢弃不能回滚，因为它们是数据一致性边界。
- 若双 lane 发布后出现数据库连接配额压力，可下调每 lane pool size，但不得合并 lane 或恢复单 Session 粗锁。
- 发布回滚必须整体回滚 Dameng Agent 与支持它的 Rust 协议客户端，避免 v2/v3 二进制不匹配；连接配置格式保持兼容。

## 21. 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| 双 lane 和换代瞬间增加 DM8 会话数 | 明确正常/峰值上限，`minimumIdle=0`，超阈值替换进程并告警 |
| 驱动线程即使 abort 后仍不退出 | 不等待旧线程；新 DataSource generation；并发未完成的隔离操作达到阈值后替换子进程 |
| 连接池破坏事务或 Session 状态 | affinity scope 固定租约，失败后整体废弃，不自动迁移 |
| 并发 RPC 导致响应串线 | 单 reader 按 id 路由；协议并发、乱序和超时测试 |
| 自动重试造成重复写入 | 只允许白名单中的只读 metadata 最多重试一次 |
| 迁移期出现两个执行入口 | 架构静态检查和明确退休门禁；最终发布前删除旧路径 |
| Hikari 默认值与版本约束不兼容 | 实施前校验实际版本，外层 DBX deadline 保持权威 |
| UI 收到旧异步事件 | Rust 和 UI 两层 generation/request token 校验 |

## 22. 设计质量门禁

实施评审必须回答并提供测试证据：

- 每个状态和资源是否只有一个 canonical owner？
- 新 Provider 是否无需修改 executor 即可接入？
- 两个 Provider 是否满足同一前置条件、后置条件和错误语义？
- 控制面接口是否只暴露控制能力，业务执行是否只依赖最小端口？
- 业务层是否依赖抽象而非 Hikari、DriverManager 等细节？
- 注释是否解释并发不变量和 DM8 异常行为，而非重复代码？
- 旧粗锁、直接 JDBC 和字符串错误推断是否已删除，而不是作为永久 fallback 留存？
- 失败路径是否全部在有限时间内收敛且不会复用可疑资源？

任一问题答案为否，均不得通过架构评审。

## 23. ADR 信号

本设计建立了持久架构边界：

- `ConnectionProvider` 是物理连接供应的权威接口。
- `JdbcExecutor` 是 JDBC Statement 执行和登记的唯一入口。
- `AgentSessionRuntime` / `SessionRuntime` 是 generation 和 operation 生命周期所有者。
- metadata/workload 双 lane 是 Agent 数据库客户端的隔离边界。
- v3 operation control 和 structured errors 是跨 Rust/Java 的公共协议契约。

实施验证上述边界且不存在未解决兼容例外后，应新增 ADR，并同步 Agent 协议基线和架构所有权记录。Spec 本身不把尚未实施的设计标记为已接受运行时事实。

## 24. 实施审计（2026-07-29）

客户端缺陷恢复链已经落地并取得直接证据：Dameng v3 强能力握手、metadata/workload 独立 Session、精确 cancel、quarantine、Hikari lease 隔离、generation 重建、迟到响应丢弃、对象树快照保护均已实现。真实 DM8 证明 `CALL SP_SLEEP(30)` 阻塞 workload 时 metadata 仍可响应，quarantine 后 replacement generation 可执行 `SELECT 1`，分页游标可跨 RPC 继续读取。

本轮同时修复了三项在实施审查中发现的所有权缺陷：

- `endOperation()` 退出 session-scoped `JdbcExecutor` 上下文后误回收分页 lease；现由 `JsonRpcServer` 保持完整生命周期上下文。
- Hikari lease invalidate 后曾存在“先普通归还、后异步淘汰”的复用窗口；现由 eviction 独占处置。
- Agent runtime 曾按历史累计 timeout/cancel 触发进程替换；现只统计尚未收到晚到响应的失控 operation。

架构退役门禁已完成：

- `DamengAgent` 与 `DamengMetadataRepository` 不再直接创建或执行 JDBC Statement；`DamengArchitectureTest` 固化零直连约束，事务与批处理 Statement 也进入统一 operation registry。
- `PoolKind::Agent` 由 `AgentConnectionPool` 持有，并通过 metadata/workload/control/compatibility 窄入口路由；SQL Agent 消费方不再暴露 `Arc<Mutex<AgentDriverClient>>`。
- v3 workload query 直接消费类型化 `AgentRpcError`，旧的 disposition 字符串反向解析桥已删除。
- 非转发 Dameng 重连先创建并验证 replacement pool，再原子交换路由；转发隧道因端点必须重置，继续采用先隔离旧 transport 再创建的明确边界。
- 完整 `cargo test -p dbx-core --lib` 仍被仓库既有 `ai_codex_cli.rs` 测试缺失导入阻断；本任务的 `cargo check`、独立恢复集成测试、Java 测试、UI 测试及真实 DM8 验收均独立通过。

因此，本 Spec 的“客户端在固定上限内恢复且无需重启”目标及第 22 节架构退役门禁均具备直接证据。客户端不承诺 DM 服务端立即终止忽略 cancel 的 SQL，但该服务端残留不再阻塞 DBX 的新 generation。

## 附录 A：TaskIntentDraft

- Outcome：Dameng 查询超时或不可取消时，DBX 在固定上限内隔离故障资源并自动恢复，无需重启。
- Goal：消除 SQL、刷新、关闭和重连之间的队头阻塞，同时保持事务和 Session 语义。
- SuccessEvidence：单元、契约、故障注入和真实 DM8 测试全部满足第 19 节。
- StopCondition：Design Spec 经用户批准；本阶段不进入实现。
- NonGoals：不承诺终止所有服务端 SQL；不同时迁移全部 JDBC Agent。
- Scope：Agent 协议、Rust runtime、Java 公共层、Dameng 驱动和相关 UI store。
- PrimaryRisks：失控线程、连接数峰值、亲和状态破坏、迟到事件污染。

## 附录 B：BaselineReadSetHint

- `docs/pips/PIP-0001-database-connection-timeout-recovery.md`
- `docs/pips/plans/2026-06-24-database-connection-timeout-recovery.md`
- `agents/common/src/main/java/com/dbx/agent/MultiSessionJsonRpcServer.java`
- `agents/common/src/main/java/com/dbx/agent/JdbcExecutor.java`
- `agents/common/src/main/java/com/dbx/agent/AgentProtocol.java`
- `agents/common/src/main/resources/agent-protocol-v2.json`
- `agents/drivers/dameng/src/main/java/com/dbx/agent/dameng/DamengAgent.java`
- `crates/dbx-core/src/db/agent_driver.rs`
- `crates/dbx-core/src/connection.rs`
- `crates/dbx-core/src/query.rs`
- `crates/dbx-core/assets/agent-protocol-v2.json`

## 附录 C：ImpactStatementDraft

- AffectedLayers：Desktop UI/store、Rust connection/query runtime、Agent JSON-RPC、Java common、Dameng driver。
- CanonicalOwners：generation 归 Session runtime；JDBC 执行归 `JdbcExecutor`；物理连接归 Provider/lease；可见快照归 generation-aware UI store。
- Invariants：控制面有限时可达；故障连接不复用；旧 generation 无写入权；亲和状态不跨连接迁移。
- Compatibility：Dameng 要求 v3；其他 Agent 可保持 v2；现有连接配置语义不变。
- Retirement：删除 Dameng 单连接字段、元数据直连 JDBC、Session 粗锁和 Dameng v2 静默降级。
- ArchitectureReviewRequired：yes。
