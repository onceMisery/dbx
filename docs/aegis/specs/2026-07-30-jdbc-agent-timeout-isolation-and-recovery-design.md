# JDBC Agent 超时隔离与恢复设计

## 1. 文档状态

- 状态：Draft，等待评审
- 日期：2026-07-30
- 架构评审：Required
- 选定方案：方案 C，公共恢复内核 + 驱动能力策略 + 分阶段启用
- 代码基线：`upstream/main@31ad4cfac`
- PR 基线：[t8y2/dbx#4914](https://github.com/t8y2/dbx/pull/4914)
- PR 合并提交：`a7a0b696054c02757d780bbb840917767a6ba738`
- 适用范围：Java JDBC Agent、Rust Agent runtime、metadata 调用链和数据库客户端恢复语义

本文取代“为 Dameng 单独复制连接池实现”的通用化方向。Dameng 是首个真实故障样本，但设计和验收面向所有 JDBC 类 Agent。

## 2. 决策摘要

PR #4914 已经建立共享 JDBC 连接池和独立 metadata Session，不能在其旁边再创建一套 Provider、Session runtime 或连接所有权。后续修复必须扩展现有 owner：

1. `JdbcConnectionPoolRegistry` 继续唯一拥有 Hikari pool、物理连接预算和 `Lease` 生命周期。
2. `AbstractJdbcAgent` 继续唯一拥有逻辑 Session 的连接亲和性，不新增平行 `JdbcSessionContext`。
3. `JdbcExecutor` 继续唯一拥有 Statement、ResultSet 和分页资源登记。
4. `MultiSessionJsonRpcServer.Session` 继续唯一拥有单 Session 串行化，但补充非阻塞 quarantine 状态机。
5. Rust `AppState`/Agent pool 继续唯一拥有 Session 路由、摘除、重建和 Agent 进程替换。

PR #4914 解决了“每个逻辑 Session 一条物理连接”和“metadata 与编辑器工作共用同一逻辑 Session”的问题，但没有完整解决“JDBC 调用永久不返回后的恢复”。本设计补齐后一部分。

## 3. PR #4914 的真实内容

### 3.1 已实现能力

PR 标题为 `feat(agent): add shared JDBC pooling and metadata sessions`，包含两个提交，最终合并为 `a7a0b6960`。其核心能力如下：

| 已有机制 | 当前实现 | 价值 |
| --- | --- | --- |
| 进程级共享池 | `JdbcConnectionPoolRegistry` 按不可变连接身份建立 Hikari pool | 多个逻辑 Session 复用有界物理连接 |
| 请求级 lease | 无状态请求借用后归还 `Lease` | 降低连接创建频率和数据库连接压力 |
| Session 亲和性 | `JdbcConnectionAffinity` + `AbstractJdbcAgent.pooledLease` | 事务、临时对象、Session SQL 和游标保持物理连接亲和 |
| 污染连接处理 | reset 失败、显式状态或失败请求触发 evict | 避免 Schema/catalog/Session 状态跨逻辑 Session 泄漏 |
| metadata 隔离 | Rust 为 metadata 任务创建短生命周期 `agentSessionId` | metadata 不再排在编辑器逻辑 Session 的锁后 |
| 取消传播 | Rust RPC timeout/cancel 发送 `cancel_session` | 对支持 `Statement.cancel()` 的驱动可终止当前 Statement |
| 弃用会话清理 | `ClientSessionPoolCleanupGuard` | future 被丢弃时仍安排 Session 清理 |
| 兼容回退 | 不支持 protocol v2 的 Agent 使用 legacy runtime | 不强制旧插件升级 |

### 3.2 已验证不变量

PR 测试覆盖：

- 同一连接身份的顺序 lease 复用物理连接。
- 每个身份的并发物理连接不超过 `maximumPoolSize`。
- 不同连接身份不共享 pool。
- 分页 cursor 持有连接直至关闭或超时回收。
- 同一逻辑 Session 的状态保持，关闭时 evict；不同 Session 不泄漏 Schema 状态。
- 不同逻辑 Session 可以并发执行。
- pooling 可通过 `DBX_AGENT_JDBC_POOL_ENABLED=false` 禁用。
- 短生命周期 metadata Session 在正常完成和取消路径均被清理。

### 3.3 当前基线不是长期裸连接模型

`AbstractJdbcAgent` 仍保留 `volatile Connection connection` 字段，但在启用共享池时，它是当前请求或亲和 lease 的兼容门面，不再表示“每个 Agent 永久持有一条裸连接”。无状态请求结束后字段会清空；状态型 Session 才保留 `pooledLease`。

因此，下阶段不应以“删除所有 `connection` 字段并重新引入 Provider”为前提。真正需要修复的是已有 lease 在不可中断调用下的退出、摘除和替换语义。

## 4. 问题结论

### 4.1 对原始分析的证实与修正

“缺少连接池”是旧实现的一个放大因素，但不是完整根因。PR #4914 已经加入连接池后，以下故障仍然可能发生：

1. 复杂查询超时只结束 Rust 调用方等待；JDBC 驱动线程可能仍阻塞。
2. `cancel_session` 不获取 Session 锁，这是正确的；但驱动可能忽略或阻塞在 `Statement.cancel()`。
3. 同一 Session 的后续请求仍在 `ReentrantLock` 后排队。
4. `close_session` 当前获取同一把 Session 锁，因此关闭和重连可能等待旧请求结束。
5. 活跃 lease 只有在原请求返回后才执行 `finishPooledRequest` 和 evict。
6. 当阻塞请求占满共享 pool，新的 metadata Session 和 replacement Session 仍会等待 pool checkout。
7. 首个 `connectionFactory.open()` 当前发生在 Hikari pool 构造之前，可能绕开 Hikari `connectionTimeout` 的保护。
8. pool checkout、`Driver.connect()`、`Connection.isValid()`、`Statement.cancel()` 和 `Connection.close()` 的 timeout 不是同一个边界，部分驱动会忽略 JDBC timeout 参数。

所以共同根因是：调用方 timeout、Session 路由失效、物理 lease 污染、异步清理和 replacement 发布尚未组成一个完整状态机。

### 4.2 “没有找到对象”的独立风险

对象树刷新失败不能等价为空结果。网络/Session/Agent 错误若在 UI 或 metadata 适配层被转换为 `[]`，用户会看到“没有找到对象”，并丢失最后一次成功快照。

修复必须同时保证：

- 只有成功响应且对象集合为空时才展示“没有找到对象”。
- timeout、连接失败、Session quarantine 和 Agent replacement 均保留最后成功快照并返回明确错误状态。
- metadata 自动重试只允许一次，且只重试明确的连接类错误；不得重放任意 SQL。

## 5. 第一性原理审查

### 5.1 不可约简目标

任意一个 JDBC 调用即使永久不返回，也不能让数据库客户端永久失去查询、metadata 刷新、关闭或手动重连能力。

### 5.2 不可破坏约束

- 事务、分页 ResultSet、临时对象、Session 变量、角色和 Schema 上下文不能静默迁移到另一条物理连接。
- 超时后的写入、DDL、事务和结果未知操作不能自动重放。
- 物理连接、Statement、逻辑 Session 和 Rust pool 各自只有一个 canonical owner。
- 未升级驱动保持 protocol v2 和现有 SQL/metadata 返回结构兼容。
- 不能假设 `cancel()`、`abort()`、`close()` 或线程中断一定有效。

### 5.3 应删除的假设

- “有 Hikari 就自然具备强恢复”。
- “`maximumPoolSize(1)` 更符合数据库客户端”。
- “RPC timeout 等于数据库操作已经停止”。
- “关闭旧 Session 后才能创建新 Session”。
- “所有 JDBC 驱动都可以使用相同的取消和连接参数”。

### 5.4 最小充分路径

不新增第二套 Provider 或 protocol v3。先在 protocol v2 和 PR #4914 的 owner 上补齐：Session quarantine、异步清理、共享池预算保留、replacement Session、饱和时 Agent 进程替换和结构化恢复结果。只有现有协议无法表达的能力才进入后续协议版本。

## 6. 方案比较

### 6.1 方案 A：恢复 Dameng 专用实现并逐驱动复制

落地单个驱动快，但会产生多套 pool、cleanup、错误分类和状态机 owner，违反开闭原则和依赖倒置原则。拒绝。

### 6.2 方案 B：所有 JDBC Agent 强制独立 pool，`maximumPoolSize(1)`

单 Session 连接亲和直观，但会重新制造 metadata 与 workload 的队头阻塞；一条不可返回连接即可占满 pool，replacement 无可用槽位。它也放弃了 PR #4914 的共享有界连接价值。拒绝。

### 6.3 方案 C：增强现有共享池和多 Session runtime

保留 PR #4914 已有共享 pool、lease 亲和和 metadata Session，新增驱动能力策略、Session quarantine 与分级恢复。方案 C 复用已有 owner、兼容风险最低，并能覆盖全部 JDBC Agent。采用。

## 7. 目标架构

```text
Desktop metadata/query caller
        |
        | timeout / cancel / manual reconnect
        v
Rust AppState + Agent pool
        | route by client session; detach and replace
        v
Agent protocol v2
        | open / cancel / close session
        v
MultiSessionJsonRpcServer
        | Session ACTIVE -> QUARANTINED -> CLOSED
        v
JsonRpcServer + JdbcExecutor
        | active Statement / cursor ownership
        v
AbstractJdbcAgent
        | request lease / pinned lease / poison-on-return
        v
JdbcConnectionPoolRegistry
        | shared Hikari pool + workload reserve + global budget
        v
JDBC Driver / Database
```

### 7.1 Owner 与退役矩阵

| 责任 | canonical owner | 本设计新增职责 | 禁止出现的第二 owner |
| --- | --- | --- | --- |
| 物理 pool 与 lease | `JdbcConnectionPoolRegistry` | 预算、角色配额、poison/指标 | 驱动私有 HikariDataSource |
| Session 物理亲和 | `AbstractJdbcAgent` | quarantine 后 poison-on-return | Rust 猜测 JDBC Connection 状态 |
| Statement/cursor | `JdbcExecutor` | 有界 cancel、未返回 operation 计数 | driver repository 私有登记表 |
| Session 路由状态 | `MultiSessionJsonRpcServer.Session` | 非阻塞 quarantine/close | `JsonRpcServer` 平行状态机 |
| replacement 与进程 | Rust `AppState`/Agent runtime | detach、重建、fail-stop | UI 自行 remove/reconnect |
| metadata 展示状态 | metadata store/caller | 失败保留最后成功快照 | 错误路径返回伪空数组 |

历史 Dameng 专用 Provider、独立 generation runtime 或 protocol v3 草案均不作为实现前置。若旧分支中存在这些实现，应在迁移前删除或改造成上述 owner 的窄策略。

## 8. Session 隔离与恢复状态机

### 8.1 Java Session 状态

```text
ACTIVE
  | request timeout/cancel
  v
CANCEL_REQUESTED
  | operation confirmed finished ------> ACTIVE
  | grace expired
  v
QUARANTINED
  | removed from routing immediately
  | active lease marked poison-on-return
  | cleanup scheduled without Session lock
  v
CLOSED
```

硬性规则：

- `cancel_session` 保持不获取 Session data lock。
- `close_session` 先从 `sessions` map 原子移除，再安排清理；不得同步等待正在执行的 Session lock。
- QUARANTINED Session 拒绝所有新 data 请求。
- 旧请求最终返回时，其 lease 必须 evict，不得重新进入共享 pool。
- 旧 JSON-RPC response 因 Rust pending waiter 已移除而被丢弃，不能写入 replacement 请求。
- Session 清理线程和队列必须有上限，不能使用无限增长的 cached executor 承担失控任务。

### 8.2 Rust replacement 流程

1. 当前 RPC 达到调用方 deadline。
2. Rust 移除 pending waiter，并发送 `cancel_session`。
3. 在默认 2 秒 `cancelGrace` 内收到“操作已结束”则返回 canceled/timeout，不重放业务 SQL；驱动 profile 可缩短但不得超过 5 秒。
4. grace 到期后，从 `AppState.connections` 摘除该 client Session pool，旧 Session 进入 QUARANTINED。
5. 使用新的 `agentSessionId` 创建 replacement Session；创建不等待旧 Session close。
6. replacement 通过连接验证后才发布给后续请求。
7. replacement checkout 因 pool 饱和失败，或失控 operation 达到阈值时，终止共享 Agent 进程并由 Rust 创建新 runtime。

手动重连直接执行第 4 至第 6 步，不等待旧请求、旧 lease 或旧 close。

### 8.3 强恢复边界

数据库客户端保证在配置的恢复预算内重新获得可操作 Session；不保证数据库服务端 SQL 一定立即终止。若驱动无法关闭 socket，Agent 进程替换是客户端侧最终隔离边界。

## 9. HikariCP 参数评审

### 9.1 `maximumPoolSize(1)` 是否合理

对 PR #4914 的共享 pool，`maximumPoolSize(1)` 不合理。

共享 pool 的 key 是不可变连接身份，不是单个逻辑 Session generation。多个编辑器 Session、短生命周期 metadata Session 和分页任务会共享同一 pool。设为 1 会导致：

- metadata 与 workload 再次串行；
- 一条阻塞连接耗尽全部容量；
- replacement Session 无法在旧 lease 未归还时建立；
- PR #4914 解决的连接复用与并发隔离退化。

连接亲和性应由 `AbstractJdbcAgent.pooledLease` 和 cursor/Session scope 保证，而不是把共享 pool 缩成 1。

只有驱动明确声明 `singlePhysicalConnectionOnly`，且接受“阻塞后直接替换 Agent 进程”时，pool size 1 才可作为驱动特例。它不能成为 JDBC Agent 默认值。

### 9.2 建议默认值

在保持 PR #4914 兼容的前提下，建议参数如下：

| 参数 | 建议默认 | 结论 |
| --- | ---: | --- |
| `maximumPoolSize` | 8/连接身份 | 保留现值；适合多 Session 客户端，但必须叠加进程级全局预算 |
| `minimumIdle` | 0 | 合理；避免桌面客户端为未使用连接长期占用数据库 Session |
| `connectionTimeout` | 30s 内层上限 | 可保留兼容值；每个调用仍必须有更短或相等的外层 deadline |
| `validationTimeout` | 5s | 合理；必须小于等于外层建连预算，不能假设驱动一定遵守 |
| `idleTimeout` | 120s | 合理；仅回收已归还的空闲连接，不影响 pinned lease |
| `maxLifetime` | 30min | 合理；Hikari 不会在借用中强制关闭连接，归还时再轮换 |
| `initializationFailTimeout` | -1 | 合理，但必须移除 Hikari 构造前的同步首次建连 |
| `isolateInternalQueries` | true | 合理；避免内部 validation 污染用户事务 |
| `keepaliveTime` | 0 | 保持关闭；避免 Hikari keepalive 与 DBX keepalive 双重所有权 |
| `poolRetireMillis` | 5min | 合理；只回收无 active lease 的整个身份 pool |

### 9.3 必须新增的预算

单个 `maximumPoolSize` 不能保证数据库客户端整体稳定，还需要：

- `metadataReserve=2`：有效值为 `min(configuredReserve, maximumPoolSize - 1)`；workload 同时最多占用 `maximumPoolSize - effectiveReserve` 个 lease，metadata 可使用保留容量。
- `globalMaximumPhysicalConnections=32/Agent runtime`：限制多个连接身份同时活跃时的总物理连接数。
- `maxQuarantinedOperations=2/连接身份`：计数达到 2 后不再创建第三个 replacement，直接替换 Agent runtime。
- `maxCleanupTasks=16/Agent runtime`：计数达到 16 后 fail-stop，禁止继续堆积不可中断清理线程。

这些值是客户端保护默认值，应允许高级配置，但不得允许 `metadataReserve >= maximumPoolSize`。当 `maximumPoolSize=1` 时 metadata reserve 为 0，并自动使用“失败即进程替换”策略。

### 9.4 真实建连必须纳入 deadline

当前 `createPoolEntry()` 先同步调用 `connectionFactory.open()`，再创建 HikariDataSource。该步骤可能永久阻塞，Hikari `connectionTimeout` 无法保护它。

修复要求：

- HikariDataSource 构造不得同步打开物理连接。
- 首次真实建连通过受监管的 checkout 路径发生。
- 外层 deadline 到期后调用方立即返回，迟到连接不得发布给已失效 Session。
- 受监管建连任务达到上限时替换 Agent 进程；Java 线程中断只作为优化，不作为正确性前提。

## 10. Session 角色与容量隔离

PR #4914 已使用短生命周期 metadata Session，但 Java 端不知道 Session 角色。协议 v2 增加可选字段 `sessionRole`：

- `workload`：编辑器查询、更新、事务、分页和批处理，默认值。
- `metadata`：对象树、补全、列/索引/外键等只读 metadata。

旧 Agent 忽略该字段，保持兼容。新 Agent 将角色传给 Registry 的 lease budget；两类 Session 仍共享同一 Hikari pool，不创建双倍 DataSource，也不破坏总 `maximumPoolSize`。

metadata 自动重试仅适用于以下同时成立的情况：

- 操作为只读 metadata 白名单；
- 错误被结构化分类为 connection/transport；
- 原 Session 已 quarantine；
- replacement 已验证；
- 当前任务此前未自动重试。

## 11. 驱动能力策略

不创建大而全的 `JdbcDriverPolicy` 接口。使用不可变、窄能力值对象 `JdbcRecoveryCapabilities`，由公共基类提供安全默认值，驱动只覆盖已验证能力：

```java
public record JdbcRecoveryCapabilities(
    CancelMode cancelMode,
    CloseMode closeMode,
    boolean singlePhysicalConnectionOnly,
    boolean supportsConnectionAbort,
    boolean supportsMetadataReserve
) {}
```

枚举只描述恢复行为，不承载 SQL 方言、URL 或凭据：

- `CancelMode.RELIABLE`：真实环境证明 `Statement.cancel()` 在 grace 内返回并终止调用。
- `CancelMode.BEST_EFFORT`：尝试 cancel，超时后 quarantine。
- `CancelMode.PROCESS_BOUNDARY`：不调用可能永久阻塞的 cancel，直接 quarantine/进程替换。
- `CloseMode.ASYNC_BOUNDED`：在受监管 cleanup executor 执行。
- `CloseMode.PROCESS_ONLY`：只依赖进程终止释放客户端资源。

默认必须是 `BEST_EFFORT + ASYNC_BOUNDED`，不能因驱动未声明就乐观认定可靠。

## 12. 错误与结果契约

在不升级 protocol version 的情况下，扩展 JSON-RPC error `data`：

```json
{
  "category": "timeout|canceled|connection|protocol|resource|sql",
  "retryable": false,
  "sessionDisposition": "keep|quarantine|replace_runtime",
  "agentSessionId": "...",
  "stage": "checkout|connect|validate|execute|fetch|cancel|close"
}
```

规则：

- Rust 恢复逻辑读取结构化字段，不解析 message 文本。
- legacy Agent 缺少 `data` 时使用现有保守分类，不能自动扩大重试范围。
- SQLState `08*`、socket/transport 失败、连接 validation 失败默认 quarantine。
- SQL 语法、权限和约束错误默认保留 Session，除非驱动明确报告连接状态未知。
- 对象树调用必须区分 `success(empty)` 与 `failure`。

## 13. 并发与资源治理

- `MultiSessionJsonRpcServer.requests` 从无界 cached thread pool 改为有界/可监管执行器。
- cleanup 使用独立有界执行器，不能占用请求线程和 maintenance 线程。
- maintenance 继续使用 `tryLock()`，不得等待繁忙 Session。
- pool Registry 暴露 active/idle/waiting/quarantined lease 计数，不暴露凭据或完整 JDBC URL。
- 日志至少记录匿名 pool key、`agentSessionId`、sessionRole、stage、elapsed、deadline 和 disposition。
- 任何日志不得记录密码、token、证书内容、完整连接串或未脱敏 SQL。

## 14. SOLID 约束

- 开闭原则：新驱动只声明 `JdbcRecoveryCapabilities` 和已有 lifecycle hook，不修改公共恢复状态机。
- 里氏替换：所有基于 `AbstractJdbcAgent` 的驱动必须满足相同 lease、quarantine、poison-on-return 和关闭后不可借出契约。
- 接口隔离：Session 路由、JDBC 执行、连接预算、驱动能力和 metadata 展示分别使用窄接口/值对象。
- 依赖倒置：driver Agent 依赖 common 生命周期；Hikari 只存在于 Registry 内，Rust/UI 不依赖 Hikari 类型。
- 单一职责：Registry 管物理资源，Agent 管亲和，Executor 管 JDBC 对象，Session 管路由状态，Rust 管 replacement。

禁止为 Dameng 或其他驱动复制 `HikariDataSource`、cleanup executor、Session 状态机或 Rust reconnect 分支。

## 15. 分阶段实施边界

### Phase 1：公共 runtime 强恢复

- Java Session 非阻塞 quarantine/close。
- lease poison-on-return。
- 移除 pool 构造前同步首次建连。
- 有界 request/cleanup executor。
- Rust detach + replacement + runtime fail-stop。
- 结构化 error data。

### Phase 2：容量与 metadata 保护

- `sessionRole` 兼容字段。
- workload lease 上限和 metadata reserve。
- Agent runtime 全局物理连接预算。
- metadata 失败保留快照与一次性安全重试。

### Phase 3：代表性驱动准入

至少覆盖：

- H2：共享 pool、亲和和故障注入样板。
- Dameng：真实复杂 SQL timeout、idle 后刷新和反复重连。
- DB2 或 Informix：传统远程 JDBC。
- Hive/Trino/Spark 之一：长查询和弱 cancel 驱动。
- Access：单连接/文件型驱动。
- TDengine：自定义建连和 native/JDBC fallback。

### Phase 4：全 JDBC Agent 启用

驱动通过公共契约和真实环境矩阵后启用能力；未验证能力保持安全默认。protocol v2 legacy fallback 仅为旧 Agent binary 保留，不新增功能。

## 16. 测试策略

### 16.1 Java 公共契约

- 阻塞 `Driver.connect()` 时，open Session 在外层 deadline 内返回。
- 阻塞 `Statement.execute()` 且 cancel 无效时，Session 在 grace 后从路由移除。
- 旧请求最终返回后 lease 被 evict，后续借出次数为 0。
- `close_session` 不等待繁忙 Session lock。
- replacement 可在旧请求仍运行时建立。
- workload 达到上限后 metadata 仍能取得保留 lease。
- request/cleanup 队列达到上限时返回 resource error 并触发 fail-stop。
- 事务、cursor、临时表和 Session SQL 始终使用原物理连接。

### 16.2 Rust/协议契约

- timeout 移除 pending waiter，迟到 response 被丢弃。
- cancel grace 到期后只摘除目标 client Session pool。
- replacement 发布前完成 validation。
- 相同 Session 的并发 replacement 只有一个成功发布。
- pool 饱和或 quarantine 阈值触发 runtime kill，所有受影响 Session 收到明确 transport error。
- legacy Agent 忽略 `sessionRole`，行为不变。
- metadata 自动重试最多一次，写入/DDL/事务重试次数为 0。

### 16.3 UI/metadata 契约

- metadata timeout、连接错误和 replacement 期间保留最后成功快照。
- 只有成功空列表显示“没有找到对象”。
- 旧请求返回不能覆盖新请求结果。
- 手动重连无需重启 DBX 即恢复。

### 16.4 真实驱动矩阵

每个启用强恢复的驱动至少验证：正常查询、复杂 SQL timeout、用户取消、idle socket、网络中断、数据库重启、分页、事务、Schema 切换、连续 20 次不可取消调用和手动重连。

## 17. 量化验收标准

1. RPC timeout 后目标 Session 在 `cancelGrace + 500ms` 内失去新请求路由资格。
2. `close_session` 和手动重连不等待旧 data 请求或 JDBC close。
3. 数据库可达且 pool 未饱和时，replacement 在 `connectionTimeout + 2s` 内验证并发布。
4. pool 饱和时不无限等待；在恢复预算内替换 Agent runtime。
5. 任意 quarantined lease 再次借出次数为 0。
6. workload 阻塞达到配置上限时，metadata reserve 仍能完成独立验证/刷新。
7. 连续 20 次不可取消调用后，请求线程、cleanup 任务、物理连接和 quarantine 计数均不超过配置硬上限。
8. 达到失控阈值后 Agent 进程被替换，不要求重启 DBX。
9. 事务、分页和 Session 状态测试证明物理连接亲和不变。
10. metadata 失败写入空数组的次数为 0；旧快照得到保留。
11. 未升级 JDBC Agent 和非 JDBC 驱动的现有行为不变。

## 18. 兼容、回滚与非目标

### 18.1 兼容

- 连接配置持久化格式不变。
- `sessionRole` 和 error `data` 均为向后兼容的可选字段。
- `query_timeout_secs = 0` 继续表示 SQL 本身不设置 Statement timeout，但客户端基础设施 deadline 仍然有界。
- pool key 的连接身份隔离规则保持不变。

### 18.2 回滚

- Phase 1 可通过 runtime feature flag 回退到 PR #4914 行为，但结构化错误读取必须保守兼容。
- `DBX_AGENT_JDBC_POOL_ENABLED=false` 仅用于驱动兼容诊断，不视为强恢复方案。
- UI 的“失败不清空快照”属于数据正确性修复，不随连接池功能回滚。

### 18.3 非目标

- 不保证数据库服务端一定杀死忽略 cancel 的 SQL。
- 不自动重放结果未知的用户操作。
- 不在本设计中修改原生 Rust 数据库驱动、KV、消息队列或非 JDBC Agent。
- 不为所有数据库设置相同网络/socket 参数；具体参数仍由 driver profile 管理。

## 19. 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| quarantine 后旧数据库 SQL 仍运行 | poison lease；异步 close/abort；阈值后终止 Agent；服务端取消仅作为驱动增强 |
| replacement 暂时增加数据库连接数 | per-identity + runtime 全局预算；quarantine 阈值；禁止无限 generation |
| metadata reserve 降低 workload 峰值 | 默认仅保留 2；可配置；优先保证客户端可恢复性 |
| runtime kill 影响同进程其他 Session | 仅在资源/线程硬阈值触发；明确 transport error；Rust 按需重建 |
| 驱动错误分类不一致 | 安全默认；结构化契约测试；真实数据库准入矩阵 |
| 兼容路径长期存在 | legacy 只读兼容，不增加新能力；发布清单记录退役条件 |

## 20. 设计质量门禁

实施评审必须全部回答“是”：

- 是否复用 PR #4914 的 Registry、Lease、Agent、Executor、Session 和 Rust pool owner？
- timeout 后是否先撤销路由资格，再异步清理？
- replacement 是否完全不等待旧 Session lock、lease close 或驱动线程返回？
- `maximumPoolSize` 是否按共享身份 pool 理解，而不是用 1 代替 Session affinity？
- 真实 `Driver.connect()` 是否受外层 deadline 和资源硬上限约束？
- quarantined lease 是否永不复用？
- metadata 是否拥有可验证的容量保留和失败快照语义？
- 事务、cursor 和 Session 状态是否保持物理连接亲和？
- 驱动差异是否只通过窄能力声明扩展？
- cleanup、request、连接和失控 operation 是否都有硬上限？

任一答案为“否”，不得宣称已解决超时后的持续不可用问题。

## 21. TaskIntentDraft

- Outcome：在 PR #4914 已有共享 JDBC pooling 和 metadata Session 基线上，形成所有 JDBC Agent 可复用的超时隔离与恢复设计。
- Goal：单个不可返回 JDBC 调用不能永久传播到后续查询、metadata、关闭和手动重连。
- SuccessEvidence：第 16 节测试和第 17 节量化标准全部通过。
- StopCondition：本文完成用户评审；本阶段不进入实现计划或代码变更。
- NonGoals：不迁移非 JDBC 驱动，不保证服务端 SQL 必然终止，不自动重放未知结果操作。
- ArchitectureReviewRequired：yes。

## 22. BaselineReadSetHint

- PR #4914 合并提交 `a7a0b6960`
- `agents/common/src/main/java/com/dbx/agent/JdbcConnectionPoolRegistry.java`
- `agents/common/src/main/java/com/dbx/agent/JdbcConnectionAffinity.java`
- `agents/common/src/main/java/com/dbx/agent/AbstractJdbcAgent.java`
- `agents/common/src/main/java/com/dbx/agent/JdbcExecutor.java`
- `agents/common/src/main/java/com/dbx/agent/JsonRpcServer.java`
- `agents/common/src/main/java/com/dbx/agent/MultiSessionJsonRpcServer.java`
- `agents/common/src/test/java/com/dbx/agent/JdbcConnectionPoolingTest.java`
- `agents/docs/agent-protocol-v2.md`
- `crates/dbx-core/src/db/agent_driver.rs`
- `crates/dbx-core/src/connection.rs`
- `crates/dbx-core/src/schema.rs`

## 23. ImpactStatementDraft

- AffectedLayers：Java common JDBC runtime、Java JDBC driver lifecycle hooks、Rust Agent runtime/pool、metadata 调用链和对象树状态。
- CanonicalOwners：Registry 管物理资源；AbstractJdbcAgent 管亲和；JdbcExecutor 管 JDBC 对象；Session 管路由状态；Rust AppState 管 replacement。
- Invariants：旧 Session 先失去路由；故障 lease 不复用；replacement 不等待旧 close；事务/游标不跨连接；metadata 失败不伪装为空。
- Compatibility：protocol v2 可选字段扩展；旧 Agent 和非 JDBC 驱动行为不变。
- Retirement：Dameng 专用恢复 owner、同步 close/reconnect 等待、无界请求/清理线程和字符串错误恢复。
- PrimaryRisks：不可中断驱动线程、临时连接增长、共享 runtime fail-stop 影响面和驱动真实行为差异。
