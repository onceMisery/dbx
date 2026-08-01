# 基于 Agent v2 的结构化后端错误契约重设计

## 1. 状态与结论

- 状态：待用户确认
- 修订日期：2026-08-01
- 本文是实现前 Design Spec，不代表代码已经实现，也不构成已接受 ADR。
- 结论：在 Agent Protocol v2 上增加可选的 `structured_error_v1` capability，并在协议错误之外建立 DBX 内部错误类型和公共 `BackendError v1`。不创建 Agent Protocol v3。

本文替换此前把 session generation、operation control 和结构化错误绑定到“协议 v3”的设计。那些文档可以作为历史设计输入，但不能证明当前运行时存在 v3；实现以代码和 v2 协议资产为准。

## 2. 当前基线

### 2.1 已验证事实

当前 Agent 运行时有两条协议代际：

- v1：单进程、单连接生命周期。
- v2：多逻辑 Session，握手返回 `protocolVersion=2` 和 `multi_session`。

基线文件：

- `agents/common/src/main/java/com/dbx/agent/AgentProtocol.java`
- `agents/common/src/main/java/com/dbx/agent/AgentRpcError.java`
- `agents/common/src/main/resources/agent-protocol-v2.json`
- `agents/docs/agent-protocol-v2.md`
- `crates/dbx-core/assets/agent-protocol-v2.json`
- `crates/dbx-core/src/db/agent_driver.rs`

当前没有可执行的 Agent Protocol v3 握手、v3 请求字段、v3 capability 或 v3 错误类型。旧 Spec/ADR 中的 v3 表述与当前代码冲突，必须在后续架构清理中标记为历史记录或废止记录。

### 2.2 当前问题

1. Java `AgentRpcError` 已在 JSON-RPC `error.data` 中输出 `category`、`retryable`、`sessionDisposition`、`stage` 和可选 `agentSessionId`，但没有错误契约版本，也没有操作结果语义。
2. Rust `decode_agent_response` 把 `error.data` 拼接进 `DBX_AGENT_ERROR_DATA:` 字符串，query、schema、connection 再从字符串反向解析恢复信息。
3. Rust 本地超时、取消、进程 EOF 和非 Agent 驱动仍主要是 `String`，没有统一的错误来源和结果语义。
4. Tauri/HTTP 迁移命令以及 `ExecuteMultiResult`/批处理进度会把错误压成字符串或伪造 `Error` 列；前端再从 `Error.message` 和驱动文案推断展示。
5. 这种链路无法保证国际化稳定、恢复策略可验证，也无法判断一次写操作的结果是否未知。

## 3. 目标与非目标

### 3.1 目标

1. 保持 `protocolVersion=2`，通过 `structured_error_v1` 协商错误 data 的能力和版本。
2. 在 Rust JSON-RPC 解码边界完成一次结构化解析，向恢复消费者提供强类型错误。
3. 将 Agent 的事实/建议与 Rust 的最终恢复动作分开；恢复逻辑不再解析 `Display`、驱动文本或本地化文案。
4. 为迁移范围内的 JDBC Agent 错误提供稳定、可本地化的 `DBX-JDBC-*` code。
5. 让迁移命令的 Tauri rejection、HTTP error body 和前端 normalize 使用同一 `BackendError v1` 形状。
6. 让多语句结果和批处理进度携带结构化错误，同时保持现有客户端 wire shape 的兼容性。
7. 为旧 Agent、非 Agent 和历史字符串保留一个明确的 compatibility adapter，并定义退役门禁。

### 3.2 非目标

- 不引入 Agent Protocol v3，也不实现 generation、operation control、lane 或新的 v3 RPC。
- 不在本次迁移所有数据库驱动；非 Agent 错误只接入统一 legacy adapter。
- 不因为 `retryable=true` 自动重放用户 SQL、写入、DDL、事务或批处理。
- 不把原始 JDBC detail、Session ID、凭据或完整 SQL 作为普通用户文案。
- 不修改通用 `QueryResult` 的数据库结果语义，不把真实名为 `Error` 的列视为执行失败。

## 4. 第一性原理与所有权

### 4.1 不变量

- 错误事实只有一个结构化来源；字符串只能是日志或兼容载体。
- Agent 只报告事实和 disposition 建议；Rust 决定是否保留、隔离或替换 Runtime。
- “操作未开始”与“操作结果未知”必须可区分；未知结果永远不能被标记为安全重放。
- 内部 Agent 错误、公共 BackendError 和前端本地化文案是三个层次，不能互相替代。
- 兼容解析只允许存在于一个 adapter；业务模块禁止新增正则或 marker 解析。

### 4.2 所有权矩阵

| 信息 | 唯一所有者 | 兼容载体 | 退役条件 |
| --- | --- | --- | --- |
| Agent 故障事实 | Java `AgentRpcError`/Agent 协议 | 旧 `error.data`、旧文本 | 所有目标 Agent 通过 capability fixture |
| 协议解码 | Rust `agent_driver` adapter | legacy adapter | 所有 Agent 消费者只接强类型错误 |
| Session/Runtime 恢复 | Rust `RecoveryPolicy` | 无 | query/schema/connection/keepalive 全部迁移 |
| 用户错误 code | Rust `BackendErrorCatalog` | `DBX-LEGACY-0001` | 迁移范围已覆盖且有契约测试 |
| 本地化 | 前端 i18n | 字符串 fallback | 支持 locale 完整性测试通过 |
| 原始 detail | Rust 诊断层 | 受控 detail | 不向分类和恢复逻辑开放 |

## 5. 方案比较与选择

### 5.1 A：只在 Rust 解析现有字段

改动小，但没有能力协商和契约版本；旧 Agent 的偶然字段无法与保证过的字段区分。拒绝作为正式方案。

### 5.2 B：v2 capability + 独立错误契约版本（采用）

保持协议 v2，握手可声明 `structured_error_v1`，`error.data.contractVersion=1` 定义字段版本。新 Agent 严格输出，旧 Agent 由单一 adapter 保守降级。它解决当前问题且不制造不存在的协议代际。

### 5.3 C：新建 Agent Protocol v3

需要新的握手、请求路由、发布和回滚矩阵，当前没有运行时基础，也不是结构化错误的必要条件。拒绝；未来若确实需要 operation control，再单独立项。

## 6. Agent v2 结构化错误契约

### 6.1 capability 语义

`structured_error_v1` 表示：该运行时对所有承诺的 Agent 错误响应遵守本节 schema，且已通过跨语言 fixture。它不是“协议 v3”，不改变 v2 的方法和 Session 语义。

Java 公共 `AgentProtocol` 可以提供 capability 常量，但只有使用公共 `AgentRpcError` 并通过契约测试的 Agent 才能在 handshake 中声明它。非 Java 或自定义 handshake 的 Agent 必须显式 opt-in；不能因为 `protocolVersion=2` 自动获得该能力。

### 6.2 JSON-RPC error.data v1

```json
{
  "contractVersion": 1,
  "category": "connection",
  "retryable": false,
  "sessionDisposition": "quarantine",
  "stage": "execute",
  "operationOutcome": "unknown",
  "agentSessionId": "session-id",
  "sqlState": "08006",
  "vendorCode": -6007,
  "exceptionClass": "java.sql.SQLRecoverableException"
}
```

| 字段 | 规则 |
| --- | --- |
| `contractVersion` | 必填整数，当前固定为 `1`；缺失或非 1 不得当作严格 v1 |
| `category` | `connection`、`sql`、`resource`、`protocol`、`timeout`、`canceled` |
| `retryable` | Agent 的内部提示；不能单独触发 SQL 重放，也不直接驱动 UI |
| `sessionDisposition` | `keep`、`quarantine`、`replace_runtime`，是建议不是命令 |
| `stage` | `request`、`checkout`、`connect`、`validate`、`execute`、`fetch`、`cancel`、`close` |
| `operationOutcome` | 错误时为 `not_started` 或 `unknown`；无证据必须使用 `unknown` |
| `agentSessionId` | 有 Session 的请求必须存在，并由 Rust 与当前路由 Session 校验；`request`/无 Session 可省略 |
| `sqlState` | 可选 JDBC SQLState，只作诊断 |
| `vendorCode` | 可选 JDBC vendor code，只作诊断 |
| `exceptionClass` | 可选 Java 类名，只作诊断，限制长度和字符集 |

未知附加字段允许存在；未知枚举、缺少必填字段、类型错误或非法组合均为 contract violation。`operationOutcome` 是事实，不是授权；`retryable` 只能作为日志/策略输入，不能被前端当作按钮权限。

### 6.3 Agent 生产规则

- `connect`、`open_session`、`checkout`、`validate` 失败且没有提交用户操作时才可使用 `not_started`。
- `execute`、`fetch`、`cancel`、`close` 无法证明数据库未执行时必须使用 `unknown`。
- SQL 异常不证明写入没有发生；除非有明确证据，否则使用 `unknown`。
- `agentSessionId` 由 Rust 请求上下文校验；Agent 返回别的 Session ID 时视为协议错误。
- `detail` 不放入 v1 data；驱动文本只进入受控日志，避免把脱敏责任下推给每个 Agent。

## 7. Rust 内部类型与恢复策略

### 7.1 解码边界

`AgentRuntimeClient`/`AgentDriverClient` 的新调用走廊返回 `Result<T, AgentCallError>`。建议类型如下，名称可按现有模块调整但语义不可变：

```rust
pub enum AgentCallError {
    Structured { rpc_code: i64, message: String, context: AgentErrorContext },
    Legacy { rpc_code: Option<i64>, message: String, hints: LegacyAgentHints },
    ContractViolation { rpc_code: Option<i64>, message: String, reason: ContractViolationReason },
    Transport { message: String },
    Timeout { stage: AgentErrorStage, operation_outcome: AgentOperationOutcome },
    Canceled { stage: AgentErrorStage, operation_outcome: AgentOperationOutcome },
}
```

只有 `Structured` 承载完整 `AgentErrorContext`。旧 `error.data` 解析结果使用字段全为 `Option` 的 `LegacyAgentHints`，不能伪装成 `contractVersion=1`。本地 Rust timeout/cancel 必须进入 `Timeout`/`Canceled`，不能再被当作普通 legacy 文本。

`Display` 仅用于日志和未迁移入口。query、schema、connection、keepalive 和 catalog 不得重新解析它。

### 7.2 RecoveryPolicy

Rust 将 Agent 建议转换为最终 `RecoveryDecision`：

| 条件 | 最低动作 |
| --- | --- |
| `keep` 且 Session 仍可验证 | 保留 Session，返回错误 |
| `quarantine` | 先移除该 Session 的新请求路由；不重放当前用户操作 |
| `replace_runtime` | 原子移除共享 Runtime 的全部 Agent pool，再后台有界清理 |
| `operationOutcome=unknown` | 禁止自动重放 SQL、写入、DDL、事务和批处理 |
| 仅 metadata 只读请求被隔离 | 允许最多一次新 Session 重试；用户 SQL 不适用 |
| Session ID 缺失/错配或非法组合 | fail-stop 当前路由，生成 contract violation |

Rust 可以升级为更保守动作，但不能因为 `retryable=true` 降低隔离级别。`replace_runtime` 的最终执行权属于 Rust，不属于 Agent。

### 7.3 迁移兼容

- 非 Agent 分支可暂时使用 `CoreError::Legacy(String)`，但只能在边界转换为 `BackendError`。
- 无 capability 的 Agent 进入同一个 legacy adapter；旧字段只能作为保守 hint，不能得到“v1 保证”。
- 进程 EOF、stdin/stdout 失败和响应超时属于 Rust transport/local error，不依赖驱动文本分类。

## 8. BackendError v1

### 8.1 公共 JSON 信封

公共信封字段使用 camelCase（Rust 类型使用 `#[serde(rename_all = "camelCase")]`）；`ExecuteMultiResult` 的既有 snake_case 字段不在本次改名范围内。

```json
{
  "version": 1,
  "code": "DBX-JDBC-1002",
  "messageKey": "backendErrors.jdbc.connectionInterrupted",
  "messageParams": { "stage": "execute" },
  "source": "jdbcAgent",
  "operationOutcome": "unknown",
  "detail": null,
  "diagnostics": {
    "category": "connection",
    "stage": "execute",
    "sqlState": "08006",
    "vendorCode": -6007,
    "exceptionClass": "java.sql.SQLRecoverableException"
  },
  "helpUrl": null
}
```

规则：

- `version` 固定为 `1`；`code` 由 Rust catalog 生成，发布后不可改义或复用。
- `messageKey` 由 catalog 唯一映射，前端负责文案；`messageParams` 只能是 catalog 声明的标量字段。
- `source` 为 `jdbcAgent`、`jdbcAgentLegacy` 或 `legacyBackend`。
- 不向公共信封暴露 `retryable`、`sessionDisposition`、`agentSessionId`、凭据或完整 SQL。
- `detail` 必须经过 Rust 白名单、长度限制和敏感信息过滤；HTTP 默认省略，Tauri 只在诊断区显示。
- `helpUrl` 只能由 catalog 产生，并限制为 HTTPS 官方域名；前端不得从 detail 拼接。

### 8.2 首批错误目录

| code | 适用条件 | outcome | messageKey |
| --- | --- | --- | --- |
| `DBX-JDBC-1001` | connection，request/checkout/connect/validate | `not_started` | `backendErrors.jdbc.connectionFailed` |
| `DBX-JDBC-1002` | connection，execute/fetch/cancel/close | `unknown` | `backendErrors.jdbc.connectionInterrupted` |
| `DBX-JDBC-2001` | timeout，操作未开始 | `not_started` | `backendErrors.jdbc.operationTimedOut` |
| `DBX-JDBC-2002` | timeout，结果未知 | `unknown` | `backendErrors.jdbc.operationTimedOut` |
| `DBX-JDBC-2003` | canceled | 按事实确定，默认 `unknown` | `backendErrors.jdbc.operationCanceled` |
| `DBX-JDBC-3001` | resource，未提交请求的背压 | `not_started` | `backendErrors.jdbc.busyRetryLater` |
| `DBX-JDBC-3002` | resource，Runtime 被替换 | 按事实确定 | `backendErrors.jdbc.runtimeReplaced` |
| `DBX-JDBC-4001` | sql | 继承内部 outcome | `backendErrors.jdbc.sqlFailed` |
| `DBX-JDBC-5001` | Agent protocol failure | `unknown` | `backendErrors.jdbc.protocolFailed` |
| `DBX-JDBC-5002` | v1 schema 或字段组合非法 | `unknown` | `backendErrors.jdbc.contractInvalid` |
| `DBX-JDBC-9001` | 旧 Agent 无法可靠分类 | `unknown` | `backendErrors.jdbc.legacyFailure` |
| `DBX-LEGACY-0001` | 非 Agent legacy 字符串 | `unknown` | `backendErrors.legacy` |

未覆盖的结构化组合生成 `DBX-JDBC-5002`，不得回退到原始文本猜测。catalog 必须有表驱动测试，确保 code 与 messageKey 一一对应。

## 9. Tauri、HTTP、结果和前端

### 9.1 传输边界

- 迁移范围内的 Tauri command 使用可序列化 `BackendError` rejection；未迁移 command 继续兼容 `String`。
- `crates/dbx-web` 的 `AppError` 包装同一 `BackendError`，响应为 `application/json`；HTTP status 只表达传输层结果，不改变 `code`。
- HTTP 旧纯文本、Tauri 旧 String 和原生 `Error` 由前端 adapter 统一降级，不在调用方散落解析。
- `BackendError` 类型应放在 `dbx-core` 公共错误模块，供 Tauri 与 HTTP 复用；不要复制两份 schema。

### 9.2 多语句结果和进度

保持现有 `ExecuteMultiResult` 的 `QueryResult` flatten wire shape，新增可选 sibling 字段：

```rust
pub struct ExecuteMultiResult {
    #[serde(flatten)]
    pub result: db::QueryResult,
    pub execution_error: bool,
    pub statement_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BackendError>,
}

pub struct ExecuteMultiProgress {
    pub statement_index: usize,
    pub completed: usize,
    pub total: usize,
    pub success: bool,
    pub execution_time_ms: u128,
    pub affected_rows: u64,
    pub error: Option<BackendError>,
}
```

迁移期保留 `execution_error` 和现有 `Error` 行，前端优先读取 `error`。所有前端都消费结构化字段并通过回归测试后，才删除伪造 `Error` 行；真实结果中的 `Error` 列永远按普通列处理。

Tauri/HTTP 的 progress event wrapper 继续使用现有 camelCase 序列化，并对 `error=None` 使用 `skip_serializing_if`，避免成功事件增加 `error: null`。

前端增加 `normalizeBackendError`、`BackendErrorException` 和结构化优先的 `translateBackendError`。调用方必须传入原始错误对象，禁止先执行 `e.message || String(e)` 再交给 normalize/translator。

## 10. 兼容、发布和退役

### 10.1 兼容矩阵

| Agent | capability | Rust 行为 | 保证 |
| --- | --- | --- | --- |
| 新 v2 Agent | `structured_error_v1` | 严格解析、校验、恢复策略 | 完整 v1 语义 |
| 旧 v2 Agent，带部分 data | 无 | `LegacyAgentHints` | 保守恢复，不宣称 v1 |
| 旧 v1/v2 Agent，无 data | 无 | legacy adapter | 仅保持现有兼容 |
| EOF/transport failure | 任意 | Rust local/transport error | 可隔离 Runtime，不依赖文本 |

新 Agent 发布前必须先发布能识别对象错误的 Rust/前端；旧 Rust 不认识 capability 时，新 Agent 仍保留旧字段和旧行为，不能只发送新字段。

### 10.2 实施顺序

1. 更新 v2 协议 fixture、Java `AgentRpcError` 和 capability opt-in 测试。
2. 在 Rust 解码边界建立 `AgentCallError`，覆盖 strict、legacy、transport、timeout、canceled。
3. 将 query/schema/connection/keepalive 的 Agent 恢复消费者迁移到 `RecoveryPolicy`。
4. 增加 `BackendErrorCatalog`，再接入 Tauri/HTTP 和前端 normalize。
5. 扩展 `ExecuteMultiResult`、progress、query store 和 locale，并保留旧字段。
6. 满足退役门禁后删除 marker parser 和伪 `Error` 行。

### 10.3 `DBX_AGENT_ERROR_DATA:` 退役门禁

以下条件全部满足后才删除 marker：

1. Agent call 边界不再以 `String` 传递结构化错误。
2. query、schema、connection、keepalive、pool detach 不再调用 marker parser。
3. 新 v2 Agent 的 capability、schema、Session ID 校验和真实运行时测试通过。
4. 旧 Agent 只由一个 adapter 生成 `AgentCallError`，业务模块无 marker 依赖。
5. Tauri/HTTP/frontend 测试证明结构化对象不会被压扁成字符串。

发现 contract violation 或敏感 detail 泄漏时，停止声明 `structured_error_v1`，回退到 adapter；不得重新增加消费模块文本规则。

## 11. 安全与可观测性

- Rust 统一限制 `detail`、`sqlState`、`vendorCode`、`exceptionClass` 的长度和字符集。
- 禁止密码、Token、私钥、完整 JDBC URL 参数和完整用户 SQL 进入 BackendError。
- 日志默认记录 `code/category/stage/operationOutcome/recoveryDecision`，不记录 raw detail。
- `agentSessionId` 只用于内部关联和路由校验，不进入普通用户错误。
- `retryable` 与 `operationOutcome` 分开记录；日志和 UI 不把它们合并成“可安全重试”。

## 12. 测试与验收

### 12.1 协议和 Rust

- v2 handshake capability fixture；strict v1、legacy、未知字段、未知枚举、缺失字段和非法组合测试。
- Java 序列化覆盖 category、disposition、stage、outcome、SQLState 和 vendor code。
- Rust 测试 Session ID 错配、EOF、超时、取消、Runtime 替换，以及 `retryable=true` 不触发 SQL 重放。
- `RecoveryPolicy` 表驱动测试：connection/sql/resource/protocol 与 keep/quarantine/replace 的安全组合。
- 静态检查保证 marker parser 只存在于 compatibility adapter。

### 12.2 传输和前端

- Tauri rejection 与 HTTP JSON error 的字段快照一致，HTTP status 单独测试。
- normalize 覆盖 BackendError、旧 JSON、纯文本、Tauri String 和原生 Error。
- `ExecuteMultiResult.error`/progress error 与旧 `Error` 列共存；真实 `Error` 列不误判。
- `en`、`es`、`it`、`ja`、`ko`、`pt-BR`、`zh-CN`、`zh-TW` 的 messageKey 完整性测试。
- `helpUrl` 的 HTTPS/域名白名单和 detail 脱敏测试。

### 12.3 真实 Agent 回归

至少覆盖达梦、DB2、TDengine 的连接中断、checkout 背压、超时、取消、metadata 重试和 Runtime 替换；同时验证旧 v1/v2 Agent 仍可工作，未知结果不会自动重放写操作。

### 12.4 验收标准

1. 运行时代码和新文档不再假设 Agent Protocol v3；结构化错误只通过 v2 capability 协商。
2. Rust 恢复消费者不从驱动文本、`Display` 或 marker 提取字段。
3. 严格 v1 错误只解码一次，非法组合不会直接执行危险 disposition。
4. `operationOutcome=unknown` 时不自动重放 SQL，UI 不宣称“安全重试”。
5. Tauri/HTTP 迁移命令可被 `normalizeBackendError` 保留为对象。
6. 迁移范围内 JDBC Agent 错误都有稳定 code；无法分类旧 Agent 使用 `DBX-JDBC-9001`。
7. 多语句和 progress 错误不再必须依赖 `rows[0][0]` 或字符串字段。
8. 支持 locale 有主文案，detail 不参与分类且敏感信息测试通过。
9. marker 退役后旧 Agent 仍由 adapter 兼容，业务模块无 marker 依赖。
10. 达梦、DB2、TDengine 的连接恢复和 metadata 重试没有回归。

## 13. 工作草案

### TaskIntentDraft

- 结果：在真实 v2 基线上建立可协商、可版本化、可安全恢复、可国际化的结构化错误通道。
- 成功证据：v2 capability fixture、强类型恢复传播、统一 BackendError wire contract、locale 完整性和真实 Agent 回归。
- 停止条件：第 12.4 节全部满足；本 Spec 仍未被误当作已接受 ADR。
- 非目标：Agent Protocol v3、全驱动迁移、未知 SQL 自动重放、帮助网站。

### BaselineReadSetHint

- `agents/common/src/main/java/com/dbx/agent/AgentProtocol.java`
- `agents/common/src/main/java/com/dbx/agent/AgentRpcError.java`
- `agents/common/src/main/resources/agent-protocol-v2.json`
- `agents/docs/agent-protocol-v2.md`
- `crates/dbx-core/assets/agent-protocol-v2.json`
- `crates/dbx-core/src/db/agent_driver.rs`
- `crates/dbx-core/src/query.rs`
- `crates/dbx-core/src/schema.rs`
- `crates/dbx-core/src/connection.rs`
- `src-tauri/src/commands/query.rs`
- `crates/dbx-web/src/error.rs`
- `apps/desktop/src/lib/backend/http.ts`
- `apps/desktop/src/lib/backend/tauri.ts`
- `apps/desktop/src/i18n/backend-errors.ts`

### ImpactStatementDraft

- 影响层：Java v2 Agent、Rust Agent driver/query/schema/connection、Tauri/HTTP、TypeScript backend adapter、批处理事件和 i18n。
- 新 owner：Agent 产出事实，Rust 校验/恢复/catalog，前端本地化，legacy adapter 单点兼容。
- 关键边界：未知结果不重放；raw detail 不分类；`protocolVersion=2` 保持不变；现有 result flatten shape 保持。
- ADR 信号：实现完成后应修订旧文档中“v3 已实施”的表述；在此之前不把旧 ADR 作为当前运行时事实。

## 14. ADR 信号

本 Spec 涉及 v2 capability、跨 Java/Rust/TypeScript 的公共错误契约、Rust recovery/catalog owner、Tauri/HTTP wire contract 和 marker 退役门禁。实现并验证后应另立或修订 ADR；本文件本身只固定待审设计和验收标准。
