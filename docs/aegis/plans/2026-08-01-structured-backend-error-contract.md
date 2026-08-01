# Goal

在现有 Agent Protocol v2 上实现已确认的结构化后端错误契约：Agent 错误可协商、Rust 恢复逻辑强类型、Tauri/HTTP 使用统一 `BackendError v1`，前端保留结构化对象并本地化，同时兼容旧 Agent 和旧字符串路径。

# Architecture

- Java Agent owns JDBC facts and emits `error.data` v1 only when `structured_error_v1` is explicitly advertised.
- Rust `agent_driver` is the single decode/compatibility boundary. `RecoveryPolicy` owns Session/Runtime action and never parses display strings.
- Rust `BackendErrorCatalog` owns stable error codes and public JSON shape.
- Tauri and HTTP serialize the same `BackendError`; frontend `normalizeBackendError` owns object preservation and i18n fallback.
- Existing `ExecuteMultiResult` flatten shape remains; structured `error` is an optional sibling field.

# Tech Stack

- Java 21 / Gradle / JUnit 5 for Agents.
- Rust 2021 / Cargo / Tokio / serde for `dbx-core`, Tauri and `dbx-web`.
- TypeScript / Vue / Vitest for desktop frontend.

# Baseline/Authority Refs

- `docs/aegis/specs/2026-07-31-structured-backend-error-contract-design.md`
- `agents/common/src/main/java/com/dbx/agent/AgentProtocol.java`
- `agents/common/src/main/java/com/dbx/agent/AgentRpcError.java`
- `agents/common/src/main/java/com/dbx/agent/MultiSessionJsonRpcServer.java`
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

# Compatibility Boundary

- `protocolVersion` remains `2`; no v3 handshake or request fields.
- Capability is opt-in. Old Agent and non-Agent strings go through one adapter and never receive strict v1 guarantees.
- Existing Tauri commands and frontend callers may remain string-compatible until explicitly migrated.
- Existing result columns and `execution_error` remain during migration; `Error` row removal is gated by consumer tests.
- Unknown operation outcomes never trigger automatic SQL, DDL, transaction or batch replay.

# Verification

- Java protocol and error-data contract tests plus Gradle common tests.
- Rust unit/integration tests for strict decode, legacy adapter, recovery policy, catalog, query propagation and wire snapshots.
- Frontend Vitest tests for normalization, translation, HTTP/Tauri object preservation and multi-result compatibility.
- `cargo fmt --check`, focused `cargo test`, frontend `pnpm typecheck`, focused `pnpm vitest run`, and final workspace checks.

# Implementation Slices

## Slice 1: v2 capability and Agent/Rust error corridor

Files:

- `agents/common/src/main/java/com/dbx/agent/AgentProtocol.java`
- `agents/common/src/main/java/com/dbx/agent/AgentRpcError.java`
- `agents/common/src/main/java/com/dbx/agent/MultiSessionJsonRpcServer.java` if handshake opt-in requires it
- v2 JSON assets and `agents/docs/agent-protocol-v2.md`
- `agents/common/src/test/java/com/dbx/agent/AgentProtocolTest.java` or existing protocol test file
- `agents/common/src/test/java/com/dbx/agent/AgentRpcErrorTest.java` or existing error test file
- `crates/dbx-core/src/db/agent_driver.rs`

Actions:

1. Add `structured_error_v1` constant and explicit capability advertisement path. Do not make unrelated native or custom handlers claim it.
2. Extend `AgentRpcError` with `contractVersion`, `operationOutcome`, and bounded diagnostic fields. Preserve old fields for old-host compatibility.
3. Define Rust enums and `AgentCallError`, `LegacyAgentHints`, strict validation, expected-session validation, local timeout/cancel variants, and one legacy adapter.
4. Keep a temporary string formatter only at the un-migrated boundary; new typed calls must not expose marker parsing to consumers.
5. Add Java/Rust fixtures for strict v1, legacy data, missing fields, unknown enum, unknown extra field, session mismatch and transport/local failures.

Verification:

- `./gradlew :common:test --tests '*Agent*'` from `agents`.
- `cargo test -p dbx-core db::agent_driver::tests --no-default-features`.
- `cargo fmt --check` for Rust and fixture JSON validation.

Commit after verification: `feat(agent): 增加 v2 结构化错误契约`.

## Slice 2: Rust recovery policy and BackendError catalog

Files:

- `crates/dbx-core/src/db/agent_driver.rs`
- new `crates/dbx-core/src/backend_error.rs` (or an equivalent single public error module)
- `crates/dbx-core/src/lib.rs`
- `crates/dbx-core/src/query.rs`
- `crates/dbx-core/src/schema.rs`
- `crates/dbx-core/src/connection.rs`
- focused Rust tests in the owning modules

Actions:

1. Introduce `RecoveryPolicy`/`RecoveryDecision` and validate cross-field category, stage, disposition and outcome combinations. Agent disposition remains advisory.
2. Convert Agent query/schema/connection/keepalive recovery consumers from marker/category string parsing to typed errors. Keep non-Agent `CoreError::Legacy(String)` only at the boundary.
3. Implement `BackendError v1`, catalog mappings `DBX-JDBC-1001` through `9001` and `DBX-LEGACY-0001`, scalar parameter allowlists, safe diagnostics and bounded detail.
4. Preserve metadata-only retry behavior at most once; prohibit user-operation replay when outcome is unknown.
5. Keep marker parsing exclusively inside the compatibility adapter and add a static/reference test for that boundary.

Verification:

- `cargo test -p dbx-core --no-default-features db::agent_driver`.
- `cargo test -p dbx-core --no-default-features query:: schema:: connection:: backend_error::` using the repository's supported filter form.
- `cargo clippy -p dbx-core --no-default-features --all-targets -- -D warnings` for touched modules if baseline permits.
- Snapshot/serialization tests for catalog, redaction, session mismatch and no SQL replay.

Commit after verification: `feat(core): 引入结构化错误恢复策略和错误目录`.

## Slice 3: Tauri/HTTP, multi-result events and frontend

Files:

- `src-tauri/src/commands/query.rs` and migrated query command error signatures
- `crates/dbx-web/src/error.rs` and affected query routes
- `apps/desktop/src/lib/backend/http.ts`
- `apps/desktop/src/lib/backend/tauri.ts`
- `apps/desktop/src/lib/backend/errorUtils.ts` or the existing backend utility owner
- `apps/desktop/src/i18n/backend-errors.ts` and all supported locale message files
- `apps/desktop/src/types/database.ts`
- `apps/desktop/src/stores/queryStore.ts`
- `apps/desktop/src/components/grid/DataGrid.vue` and focused frontend tests

Actions:

1. Serialize migrated Tauri rejections and HTTP failures as the shared camelCase `BackendError v1`; retain legacy string fallback at the adapter edge.
2. Add optional structured `error` to `ExecuteMultiResult` and progress wrappers without changing existing snake_case result fields or successful event shape.
3. Add `BackendErrorException`, `normalizeBackendError` and structured-first translation. Update migrated callers to pass original error objects instead of `e.message || String(e)`.
4. Add catalog locale keys and parameter validation for all eight supported locales.
5. Keep old `Error` result rows as compatibility input while making structured fields authoritative.

Verification:

- `cargo test -p dbx-web --no-default-features` for error response snapshots and affected routes.
- `pnpm typecheck`.
- `pnpm vitest run apps/desktop/src/i18n apps/desktop/src/lib/backend apps/desktop/src/stores/__tests__/queryStore*` with repository-supported Vitest paths.
- Wire tests prove Tauri/HTTP shape parity, legacy fallback, progress omission of null errors and real `Error` columns.

Commit after verification: `feat(ui): 接入统一结构化后端错误展示`.

## Slice 4: compatibility retirement and developer guide

Files:

- remaining legacy adapter/static checks and tests
- `docs/backend-error-handling.md`
- `docs/aegis/work/2026-08-01-structured-backend-error-contract/*`
- historical v3 docs only if explicitly marked as historical; do not rewrite unrelated architecture claims opportunistically

Actions:

1. Verify all marker consumers are gone except the adapter and document the exact retirement gate.
2. Add a concise developer guide covering layer ownership, `AgentCallError`, `BackendError`, code naming, `messageKey`, params, outcome semantics, redaction and forbidden string-based recovery.
3. Run the complete targeted test matrix and update evidence, drift and residual-risk records.
4. Recheck Spec acceptance criteria and identify any real-environment coverage that remains unavailable.

Verification:

- `rg -n "DBX_AGENT_ERROR_DATA|agent_rpc_error_category|agent_session_disposition"` shows only the adapter and explicitly retained fixtures.
- `python C:\Users\miracle\.codex\aegis\scripts\aegis-workspace.py bundle --root D:\code\github\dbx --work 2026-08-01-structured-backend-error-contract`.
- `python C:\Users\miracle\.codex\aegis\scripts\aegis-workspace.py check --root D:\code\github\dbx`.
- Final Rust, Java and frontend commands from the evidence bundle.

Commit after verification: `docs(error): 增加后端异常处理与错误码规范`.

# Risks and Rollback

- Capability rollout is reversible by removing `structured_error_v1` from handshake advertisement while retaining old data fields.
- BackendError migration is command-scoped; an untouched command may continue returning a string through the legacy adapter.
- Runtime replacement policy can affect shared pools; typed policy tests and existing detach/replace tests are mandatory before changing consumers.
- Real Dameng/DB2/TDengine environments may be unavailable. Record this as residual risk rather than weakening the unknown-outcome invariant.

# Retirement

`DBX_AGENT_ERROR_DATA:` and synthesized `Error` rows are retained only until the Spec's consumer and wire tests pass. Their deletion must be a separate verified change within Slice 4, not an incidental cleanup during an earlier slice.
