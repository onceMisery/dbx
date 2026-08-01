# 后端异常处理与错误码规范

本文说明 DBX 后端错误的分层职责和新增错误码时的约束。目标是让恢复逻辑可验证、对外契约稳定，同时继续兼容旧 Agent 和未迁移的字符串接口。

## 分层职责

1. Agent 只报告事实：`category`、`stage`、`operationOutcome`、`sessionDisposition` 和 JDBC 诊断信息。
2. Rust `AgentCallError` 负责严格解码 Agent v2 的 `structured_error_v1`；旧 Agent 只经过一个 compatibility adapter。
3. `RecoveryPolicy` 决定保留 Session、隔离 Session、替换 Runtime，或对只读 metadata 最多重试一次。
4. `BackendError` catalog 负责稳定 `code`、`messageKey`、参数白名单和安全诊断字段。
5. Tauri/HTTP 只序列化 `BackendError`；前端使用 `normalizeBackendError` 和 `translateBackendError` 展示本地化文案。

结构化错误是 Agent Protocol v2 的可选 capability，不存在本功能专用的 Protocol v3。

## 后端代码怎么写

Agent 调用路径应尽量返回 `Result<T, AgentCallError>`。业务代码不得从 `Display`、驱动消息或 `DBX_AGENT_ERROR_DATA` 中重新提取恢复字段。

```rust
let result = client.execute_query_typed_with_timeout(params, timeout).await;
if let Err(error) = &result {
    let decision = RecoveryPolicy::decide(error, RecoveryScope::UserOperation);
    // 执行隔离或 Runtime 替换；不要重放当前用户操作。
}
```

非 Agent 的旧接口可以在传输边界使用：

```rust
let public_error = BackendError::from_legacy_string(&message);
```

不要在 query、schema、connection、keepalive 或 UI 中新增基于错误文本的连接分类规则。旧 Agent 文本兼容只能放在 `agent_driver` 的单一 adapter 中。

## 恢复规则

- `operationOutcome=unknown`：禁止自动重放 SQL、写入、DDL、事务和批处理。
- 用户操作：即使 `retryable=true`，也只执行 Session/Runtime 恢复并向用户返回错误。
- 只读 metadata：仅 connection + quarantine 可新建 Session 重试，最多一次。
- `replace_runtime`：移除共享同一 Runtime 的路由；最终决定权在 Rust，不在 Agent。
- contract violation、timeout、cancel：至少隔离当前 Session；stale Session 结果不得移除新的路由代际。

## 错误码规范

格式为 `DBX-<DOMAIN>-<NNNN>`：

- `DBX-JDBC-1xxx`：连接建立或连接中断。
- `DBX-JDBC-2xxx`：超时和取消。
- `DBX-JDBC-3xxx`：资源压力和 Runtime 替换。
- `DBX-JDBC-4xxx`：数据库 SQL 错误。
- `DBX-JDBC-5xxx`：协议或契约错误。
- `DBX-JDBC-9xxx`：旧 Agent 无法可靠分类。
- `DBX-LEGACY-0001`：非 Agent 或未迁移字符串错误。

新增错误码时：

1. 在 `crates/dbx-core/src/backend_error.rs` 的 catalog 中增加唯一 code、`messageKey` 和参数声明。
2. code 发布后不得改义、复用或根据 locale 改变。
3. `messageParams` 只能使用 catalog 声明的 string/number/boolean 标量；禁止传入 SQL、URL、凭据或任意对象。
4. 为八个 locale 增加相同 `messageKey`，并扩展 catalog 完整性测试。
5. 增加 Rust 映射/序列化测试和前端 normalize/翻译测试。

## 安全与展示

- 公共错误不得包含密码、token、私钥、完整 JDBC URL、完整 SQL 或 `agentSessionId`。
- `detail` 只用于受控诊断，必须通过长度、字符集和敏感内容过滤；分类永远不依赖 `detail`。
- HTTP status 表示传输结果，不能替代或改变 `BackendError.code`。
- 多语句结果中的 `error` 是权威字段；旧 `Error` 行仅用于兼容，真实查询结果里名为 `Error` 的普通列不能被当作失败。
- 前端 catch 到错误后应把原始对象传给 `translateBackendError`，不要先执行 `e.message || String(e)`。

## 提交前检查

```text
cargo fmt --all -- --check
cargo test -j 1 -p dbx-core --no-default-features --lib backend_error::tests
cargo test -j 1 -p dbx-core --no-default-features --lib agent_recovery::tests
cargo check -j 1 -p dbx-web --no-default-features
cargo check -j 1 -p dbx --no-default-features
pnpm typecheck
pnpm vitest run apps/desktop/src/i18n/__tests__/backendErrors.spec.ts
```
