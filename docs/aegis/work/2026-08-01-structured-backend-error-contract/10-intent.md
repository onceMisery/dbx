# Structured backend error contract implementation - Intent

## TaskIntentDraft

- Requested outcome: 按已确认 Spec 在现有 Agent v2 上实现结构化错误契约，并补齐开发者指南
- Goal: JDBC Agent、Rust 恢复、Tauri/HTTP 和前端共享可验证的结构化错误语义，同时保留旧 Agent 兼容
- Success evidence:
- 协议 fixture、Rust 强类型错误、RecoveryPolicy、BackendError wire contract、前端 normalize、必要测试和开发者指南
- Stop condition: 所有 Spec 验收标准有直接测试或静态证据；若外部真实数据库不可用则明确记录残余风险
- Non-goals:
- Agent Protocol v3, full non-Agent migration, automatic SQL replay, complex help site
- Scope: Java Agent v2、crates/dbx-core、src-tauri 查询命令、crates/dbx-web、apps/desktop backend/query/i18n
- Change kinds:
- contract-migration
- Risk hints:
- 共享 Agent runtime 恢复、旧字符串兼容、Tauri/HTTP wire shape 和多语句结果兼容

## BaselineReadSetHint

- docs/aegis/specs/2026-07-31-structured-backend-error-contract-design.md
- agents/common/src/main/java/com/dbx/agent/AgentProtocol.java
- agents/common/src/main/java/com/dbx/agent/AgentRpcError.java
- crates/dbx-core/src/db/agent_driver.rs
- crates/dbx-core/src/query.rs
- crates/dbx-core/src/schema.rs
- crates/dbx-core/src/connection.rs
- src-tauri/src/commands/query.rs
- crates/dbx-web/src/error.rs
- apps/desktop/src/i18n/backend-errors.ts

## ImpactStatementDraft

- Compatibility boundary: new v2 capability is opt-in; old Agent and non-Agent strings enter one legacy adapter
- Affected layers:
- Java Agent protocol
- Rust Agent/query/schema/connection
- Tauri/HTTP transport
- TypeScript frontend
- Owners:
- Agent facts: Java AgentRpcError; recovery/catalog: Rust; copy: frontend i18n; compatibility: one adapter
- Invariants:
- protocolVersion remains 2; unknown operation outcome is never auto-replayed; current result flatten shape remains
- Non-goals:
- Agent Protocol v3, full non-Agent migration, automatic SQL replay, complex help site

These records are Method Pack drafts / hints, not authoritative runtime decisions.
