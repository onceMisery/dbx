# ADR：Agent Session 代际与 JDBC 连接租约

## 状态

已接受并实施；客户端恢复与架构退役门禁均已验证。

## 背景

DM8 对部分调用（例如 `CALL SP_SLEEP`）不响应 JDBC query timeout、`Statement.cancel()`，连接 close/abort 也可能被驱动拖延。旧实现把一个长期 Connection 和一个 Session 粗锁同时当作执行与恢复边界，导致一次阻塞传播到后续查询、元数据刷新、关闭和重连。

## 决策

1. `ConnectionProvider` 是物理连接租约的唯一所有者，Hikari 与 Single Provider 遵循同一最小接口。
2. `JdbcExecutor` 和 `OperationRegistry` 是 Statement 执行、deadline 与精确 cancel 的唯一入口；Dameng 驱动特有 Statement 也必须显式登记。
3. 一个 Session 内按 `METADATA` 与 `WORKLOAD` 使用独立锁；同 lane 保持顺序，不同 lane 可并发。
4. Rust 为每个 v3 请求注入 `agentSessionId / generation / operationId / lane`。超时或取消先精确 cancel，再 quarantine；硬故障旧池立即失去路由资格，非转发 Dameng 主动重连则先验证 replacement 再原子交换，旧资源在后台有界清理。
5. 迟到响应只能完成原 JSON-RPC id；waiter 被移除后自动丢弃。新 generation 不接受旧 generation 写入。
6. Dameng 必须声明完整的 v3 四项恢复能力，缺一即拒绝连接；其他 Agent 继续使用 v2 兼容路径。
7. UI 沿用 `TreeNodeLoadRegistry` 的节点 generation 作为最终写入权限；失败不清空上次成功快照。
8. 进程替换阈值统计当前仍未返回的隔离 operation；晚到响应会释放计数，历史正常取消不得累计误杀共享 runtime。

## 设计原则映射

- 开闭原则：新增 Provider、lane handle 和协议 capability 扩展，不要求其他 Agent 改写。
- 里氏替换：业务代码只依赖 `ConnectionProvider/ConnectionLease`，Single 与 Hikari 遵循相同生命周期契约。
- 接口隔离：连接租约、Statement 执行、Session 状态、Rust 路由和 UI 写入权限分别由窄接口承担。
- 依赖倒置：DamengAgent 依赖 Provider/Lease 抽象，不依赖 HikariDataSource 的业务语义；Hikari 只是基础设施实现。

## 后果

- 即使驱动线程永久不返回，客户端也能停止等待并让旧 generation 失去路由资格。
- 不自动重放未知结果的 SQL、事务、DDL 或批处理；恢复后由用户明确重试。
- 旧 JVM 内不可中断的驱动线程仍可能暂时存在；资源阈值与进程替换是最终防线，不能把客户端恢复等同于服务端调用已经终止。

## 兼容与退役边界

- `DamengAgent` 和 `DamengMetadataRepository` 通过 `JdbcExecutor` 执行 JDBC，静态架构测试禁止业务层重新引入 Statement 直连。
- `PoolKind::Agent` 由 `AgentConnectionPool` 持有，SQL 调用按 metadata/workload/control lane 路由；Mongo/KV v2 仅通过显式 compatibility lane 访问。
- v3 workload query 使用类型化 `AgentRpcError` 决定 disposition；旧的 disposition 字符串反向解析桥已删除。
- 转发 transport 必须先重置旧隧道，不能与普通非转发 replacement 使用同一原子交换实现。
