# 后端异常处理与错误码规范

本文记录 DBX 当前已经落地的后端错误契约、恢复边界和前端展示规则。目标是让恢复逻辑依赖可验证的类型，让对外错误身份稳定，同时保留数据库服务端返回的、经过安全过滤的真实诊断信息。

本文描述的是现有实现，不引入新的 Agent Protocol V3。结构化错误是 Agent Protocol v2 的可选 capability：`structured_error_v1`。

## 分层职责

1. Agent 只报告事实：`category`、`stage`、`operationOutcome`、`sessionDisposition` 以及 JDBC 诊断字段。
2. Rust `AgentCallError` 负责解码 Agent v2 的结构化错误；旧 Agent 或旧字符串接口只经过 `agent_driver` 中的兼容 adapter。
3. `RecoveryPolicy` 根据类型化错误和操作范围决定保留、隔离 Session 或替换 Runtime；它不从错误文本推断恢复动作。
4. `BackendError` catalog 将类型化错误映射为稳定的 `code`、`messageKey`、白名单参数和安全诊断字段。
5. 查询层通过 `QueryExecutionError::into_backend_error` 生成公共错误对象；Tauri、HTTP 和多语句结果只负责携带该对象，不重复分类。
6. 前端通过 `normalizeBackendError` 和 `translateBackendError` 生成本地化摘要，并在可用时追加服务端 `detail`。

## Agent 调用契约

Agent runtime 必须完成 Protocol v2 handshake，并支持 `multi_session`。如果声明 `structured_error_v1`，`call_typed` 在 RPC 失败时返回 `AgentCallError::Structured`；否则进入 `Legacy` 兼容路径。超时、取消、传输失败和契约不满足分别使用 `Timeout`、`Canceled`、`Transport` 和 `ContractViolation`。

业务代码应使用类型化入口：

```rust
let result = client.call_typed::<Response>(method, params, timeout, cancel).await;
if let Err(error) = &result {
    let decision = RecoveryPolicy::decide(error, RecoveryScope::UserOperation);
    // 只执行 Session/Runtime 恢复，不重放当前用户 SQL。
}
```

`AgentRuntimeClient::call` 和 `AgentCallError::into_legacy_string` 仅用于尚未迁移的字符串边界。旧字符串只有在 `try_agent_error_from_legacy` 能证明其来自 Agent 调用通道时才恢复为 Agent 错误；不要在 `query`、`schema`、`connection`、`keepalive` 或 UI 中增加新的文本分类规则。

## 公共错误对象

Rust `BackendError` 的字段由 catalog 构造，字段定义如下（JSON 使用 camelCase）：

```json
{
  "version": 1,
  "code": "DBX-JDBC-4001",
  "messageKey": "backendErrors.jdbc.sqlFailed",
  "messageParams": { "stage": "execute" },
  "source": "jdbcAgent",
  "operationOutcome": "unknown",
  "detail": "relation missing_table does not exist",
  "diagnostics": {
    "category": "sql",
    "stage": "execute",
    "sqlState": "42P01",
    "vendorCode": 0,
    "exceptionClass": "java.sql.SQLException"
  }
}
```

约束：

- `version` 当前为 `1`；`code`、`messageKey` 和字段含义发布后不可复用或改义。
- `source` 只能是 `jdbcAgent`、`jdbcAgentLegacy` 或 `legacyBackend`。
- `operationOutcome` 只能是 `not_started` 或 `unknown`。结果未知时不能自动重放用户操作。
- `messageParams` 只能包含 catalog 声明的 string、number、boolean 标量，不得携带 SQL、URL、凭据或任意对象。
- Rust 字段保持私有，新增错误必须通过 catalog 构造，避免 code、key 和参数声明漂移。

## 错误码 catalog

| code | 含义 |
| --- | --- |
| `DBX-JDBC-1001` | 连接建立失败 |
| `DBX-JDBC-1002` | 已建立连接中断 |
| `DBX-JDBC-2001` | 操作超时且尚未开始 |
| `DBX-JDBC-2002` | 操作超时但结果未知 |
| `DBX-JDBC-2003` | 操作取消 |
| `DBX-JDBC-3001` | 资源繁忙，操作尚未开始 |
| `DBX-JDBC-3002` | Runtime 被替换 |
| `DBX-JDBC-4001` | 数据库 SQL 执行失败 |
| `DBX-JDBC-5001` | Agent 传输或协议失败 |
| `DBX-JDBC-5002` | Agent 错误上下文违反契约 |
| `DBX-JDBC-9001` | 旧 Agent 错误无法可靠分类 |
| `DBX-LEGACY-0001` | 非 Agent 或未迁移的字符串错误 |

新增错误码时：

1. 在 `crates/dbx-core/src/backend_error.rs` 的 catalog 中增加唯一 code、`messageKey` 和参数声明。
2. 为所有 locale 增加相同 key，并扩展 catalog 完整性测试。
3. 增加 Rust 映射和序列化测试，以及前端 normalize/翻译测试。
4. 若错误来自 Agent，先在 `AgentErrorContext` 中定义可验证的事实和合法组合，再添加 catalog 映射；不要用错误文本补分类。

## detail 与安全边界

`detail` 是服务端诊断的可选补充，不是分类依据。Agent 错误映射会调用 `safe_detail`：

- 最多保留 512 字节的 UTF-8 文本；换行、制表符和连续空白会折叠为单个空格，空内容会被丢弃。
- 过滤 JDBC URL、密码、token、授权头、密钥、Session 标识等敏感标记。
- 过滤包含 SQL 语句关键字的内容，避免把完整 SQL 回显给用户。
- `agentSessionId`、重试标记和内部恢复字段不会进入公共 envelope。
- Rust 查询执行器生成的查询超时会使用 `DBX-JDBC-2002`（阶段 `execute`）摘要，同时保留安全的超时诊断 detail；它不会作为 `DBX-LEGACY-0001` 展示。
- PostgreSQL native driver 返回的标准服务端 `ERROR:` 诊断会使用 `DBX-JDBC-4001`（阶段 `execute`）摘要并保留安全 detail；连接、超时、取消和清理错误不使用该分类。
- 超时和取消没有服务端 detail 时只返回摘要；被过滤的 detail 也不会使用替代文本冒充原始错误。

## 传输边界

### Tauri Desktop

查询命令将 `QueryExecutionError` 映射为 `BackendError`。单语句和事务查询即使通过 `execute_multi` 命令执行，`dbx-core` 也会在整个 multi-query 核心链路中保留 `QueryExecutionError`，直到 Tauri 边界才转换为 `BackendError`；不得先降级为字符串再重建 envelope。`apps/desktop/src/lib/backend/tauri.ts` 在查询失败时抛出 `BackendErrorException`，前端因此可以同时取得 `messageKey` 和安全 `detail`。

### HTTP Web

`crates/dbx-web` 的 multi-query 路由也消费 typed 核心入口，并将 `AppError` 序列化为同一套 envelope；当前响应使用 `BackendError::without_detail()`，因此 HTTP 客户端只获得稳定摘要身份，不获得 detail。HTTP status 只表示传输结果，不能替代或改变 `BackendError.code`。

### 多语句查询

`ExecuteMultiResult.error` 和进度事件中的 `error` 是权威的结构化错误字段，`execution_error` 表示该结果确实失败。已经进入 typed 通用逐语句路径的错误必须直接从 `QueryExecutionError` 生成该字段，不能从兼容字符串反向推断。MySQL 和 SQL Server 的专用 batch executor 当前仍是字符串驱动边界，只有在驱动层提供可验证的 typed failure facts 后才能迁移，不能在 query/UI 层按错误正文补分类。旧的 `Error` 行仅用于兼容；真实查询结果中名为 `Error` 的普通列不能被当作失败。

## 前端展示规则

`normalizeBackendError` 只接受完整且类型正确的 envelope；`detail` 如果存在必须是 string。`translateBackendError` 的结构化路径为：

1. 使用 `messageKey` 和 `messageParams` 生成当前 locale 的自定义摘要。
2. 若 `detail` 非空且不同于摘要，在摘要后追加空行和 detail。
3. 无法识别的旧字符串继续原样展示或按兼容 pattern 翻译。

catch 到异常时必须把原始对象传给翻译器：

```ts
translateBackendError(t, error)
```

不要先执行 `error.message || String(error)`，否则会丢失 `messageKey`、参数和服务端 detail。`BackendErrorException`、嵌套的 `{ error }`/`{ backendError }` 和普通 `Error` 都由 `normalizeBackendError` 统一处理。

## 恢复规则

- `operationOutcome=unknown`：禁止自动重放 SQL、写入、DDL、事务和批处理。
- 用户操作：即使 Agent 声明可重试，也只做 Session/Runtime 恢复并向用户返回原错误。
- 只读 metadata：只有 connection + quarantine 场景可以新建 Session 重试，最多一次。
- `replace_runtime`：移除共享同一 Runtime 的路由；最终决定权在 Rust，不在 Agent 或前端。
- contract violation、timeout、cancel：至少隔离当前 Session；旧 Session 的迟到结果不得影响新的路由代际。

## 提交前检查

```text
cargo fmt --all -- --check
cargo clippy -j 1 -p dbx-core --no-default-features --all-targets -- -D warnings
cargo test -j 1 -p dbx-core --no-default-features --lib backend_error::tests
cargo test -j 1 -p dbx-core --no-default-features --lib agent_recovery::tests
cargo check -j 1 -p dbx-web --no-default-features
pnpm typecheck
pnpm vitest run apps/desktop/src/i18n/__tests__/backendErrors.spec.ts
```

## 详情保留更新（2026-08-03）

公共错误契约仍然是 `BackendError v1`。当前已在以下边界落实统一规则：

- Rust `safe_detail` 是对外详情脱敏的唯一 owner。即使厂商诊断包含 `SELECT`、`UPDATE` 等词，也会保留安全诊断；完整 SQL 内容会被移除，包含凭据的 URL/键值片段会被掩码，Agent/Session 标识会被移除，空白会被归一化，并保持 512 字节的 UTF-8 上限。
- Axum `AppError`、查询多结果负载和 Tauri 查询进度事件使用同一个结构化 `error` 字段。`execution_error` 和旧版 `Error` 行继续作为兼容载体；当 `execution_error` 为 false 时，名为 `Error` 的真实结果列仍然是数据，不能当作失败信息。
- Desktop HTTP 失败（包括 multipart、SSE、上传、下载和 Nacos 特殊接口）必须调用 `backendResponseError`。禁止直接构造 `new Error(await response.text())`，因为这会丢失 `BackendError v1` envelope。
- Tauri 连接、传输、导入和导出边界统一将拒绝结果转换为 `BackendErrorException`。对于未知对象，在存在可用信息时提取有长度上限的 `message`、`reason` 或 `detail`；内容为空或不安全时只保留稳定摘要，不凭空编造详情。
- 前端展示后端错误时，将原始捕获对象传给 `translateBackendError(t, error)`。展示内容为本地化错误摘要，后接安全 `detail`。旧版非 i18n 页面可以使用 `formatError`，但绝不能把结构化 envelope 直接转换成 `[object Object]`。

兼容代码的退役门槛包括：不再存在直接读取 HTTP 响应文本并抛错的路径；迁移后的后端错误展示调用点不再在 `translateBackendError` 前预先提取 `.message`/`String(error)`；在所有消费者接受 `BackendError v1` 且线协议测试通过前，不移除旧字符串或旧版错误行。CI 无法接入的真实厂商数据库仍属于残余风险，必须在发布证据中明确记录。
