# ADR：Agent v2 结构化后端错误契约

## 状态

已接受并实施。

## 背景

现有运行时代码只有 Agent Protocol v2。旧设计材料曾把部分恢复与错误传输能力描述为 v3，容易让实现者把错误契约与新协议版本绑定，并在 Rust、前端和各 Agent 中重复解析错误文本。

## 决策

1. `protocolVersion` 保持 `2`；支持严格结构化错误的 Agent 通过 `structured_error_v1` capability 显式 opt-in。
2. Agent 产生事实，Rust 严格解码 `AgentCallError` 并校验字段组合和 Session ID。
3. `RecoveryPolicy` 是恢复动作的唯一决策层；用户操作未知结果时不自动重放，只有只读 metadata 最多重试一次。
4. `BackendError` catalog 是公共 code、`messageKey`、参数和安全诊断字段的唯一 owner，供 Tauri 和 HTTP 复用。
5. 前端保留结构化对象并本地化；旧字符串与旧 Agent 只能经过单一 compatibility adapter。

## 开闭原则

- 新增 Agent 错误类别时扩展协议枚举、组合校验、恢复策略和 catalog，不在业务模块增加文本规则。
- 新增公共错误码时扩展 catalog 和 locale，不改变既有 code 的含义。
- 未迁移 command 可以继续返回字符串；迁移 command 使用相同 `BackendError` schema，不复制 Tauri/HTTP 类型。

## 兼容与退役边界

- 旧 Agent 和未迁移 command 继续工作，但不获得 strict v1 保证。
- `DBX_AGENT_ERROR_DATA` 仅保留在 compatibility adapter 和显式 fixture 中；query、schema、connection、keepalive 不得直接消费字段。
- 多语句旧 `Error` 行暂时保留；结构化 `error` 已成为权威字段，待所有消费者完成迁移后单独删除。

## 对旧 ADR 的说明

本 ADR 取代 `2026-07-29-agent-session-generation-and-jdbc-leases.md` 中“结构化错误必须使用 Agent Protocol v3”的错误契约信号。该文档关于连接租约、Session 隔离、未知结果不重放和有界清理的原则仍然有效；当前运行时协议事实以本 ADR 和 Agent v2 fixture 为准。
