# Structured backend error contract implementation - Checkpoint

- Task ID: 2026-08-01-structured-backend-error-contract
- Current todo: create implementation plan and baseline checkpoint
- Active slice: planning and checkpoint setup
- Blocked on: none
- Next step: write plan, then implement Slice 1

## Checkpoint Update

- Current todo: Slice 1: implement v2 capability and typed Agent/Rust error corridor
- Active slice: Slice 1 planning complete; inspect exact test/build APIs before edits
- Completed todos:
- Re-read approved Spec and current code baseline
- Create implementation plan and work lifecycle
- Evidence refs:
- docs/aegis/specs/2026-07-31-structured-backend-error-contract-design.md
- docs/aegis/plans/2026-08-01-structured-backend-error-contract.md
- git status on enhance/exception-alert preserves unrelated user changes
- Blocked on: none
- Next step: Inspect protocol tests and implement capability/error data plus Rust decode types

## Checkpoint Update

- Current todo: Slice 2: implement typed recovery policy and BackendError catalog
- Active slice: Slice 1 verified and ready to commit
- Completed todos:
- Slice 1: v2 capability and Agent/Rust typed error corridor
- Evidence refs:
- agents: .\gradlew.bat :common:test --tests *Agent* (BUILD SUCCESSFUL)
- dbx-core: cargo test -p dbx-core --no-default-features db::agent_driver::tests (49 passed)
- cargo fmt --all -- --check and cargo check -p dbx-core --no-default-features (exit 0)
- Blocked on: none
- Next step: Commit Slice 1, then migrate recovery consumers to AgentCallError

## Checkpoint Update

- Current todo: Slice 2 complete; prepare commit and continue with transport/UI migration
- Active slice: Slice 2 verified; typed recovery policy, BackendError catalog, and Agent consumer migration are complete
- Completed todos:
- `RecoveryPolicy` moved to the dedicated `agent_recovery` module
- `BackendError v1` catalog and safe diagnostics added
- query/schema/connection/keepalive Agent recovery paths use typed decisions or the single compatibility adapter
- metadata retry remains read-only and at-most-once; user operations are never replayed
- static boundary test confirms retired marker parser consumers are absent from business modules
- Evidence refs:
- `cargo test -j 1 -p dbx-core --no-default-features --lib query::tests` (90 passed)
- `cargo test -j 1 -p dbx-core --no-default-features --lib schema::tests` (78 passed)
- `cargo test -j 1 -p dbx-core --no-default-features --lib connection::tests` (288 passed, 4 ignored)
- `cargo test -j 1 -p dbx-core --no-default-features --lib db::agent_driver::tests` (51 passed)
- `cargo test -j 1 -p dbx-core --no-default-features --lib backend_error::tests` (5 passed)
- `cargo test -j 1 -p dbx-core --no-default-features --lib agent_recovery::tests` (1 passed)
- `cargo check -j 1 -p dbx-core --no-default-features` and `cargo fmt --all -- --check` (exit 0)
- Blocked on: none
- Next step: Commit Slice 2, then implement Tauri/HTTP and frontend structured error transport

## Checkpoint Update

- Current todo: Slice 3 complete; prepare transport/UI commit
- Active slice: Shared BackendError transport and structured-first frontend handling verified
- Completed todos:
- HTTP `AppError` serializes the shared JSON BackendError envelope
- Tauri `execute_query` and `execute_multi` reject with serializable BackendError objects
- multi-statement result/progress payloads include optional structured errors while retaining legacy Error rows
- frontend normalizes HTTP/Tauri objects into `BackendErrorException` without flattening them
- all eight locales define every initial catalog message key
- queryStore preserves structured batch error details alongside compatibility display text
- Evidence refs:
- `cargo check -j 1 -p dbx-core --no-default-features` (exit 0)
- `cargo check -j 1 -p dbx-web --no-default-features` (exit 0)
- `cargo check -j 1 -p dbx --no-default-features` (exit 0; baseline dead-code warning only)
- focused core/web serialization tests passed
- `pnpm typecheck` (exit 0)
- `pnpm vitest run ...backendErrors.spec.ts ...queryStore.multiStatementError.spec.ts` (196 passed)
- Blocked on: none
- Next step: Commit Slice 3, then write the developer guide and perform final retirement/acceptance checks

## DriftCheckDraft

- Scope status: Slice 1 stayed within Agent v2 and typed Rust corridor
- Compatibility status: protocolVersion remains 2; structured_error_v1 is explicit JDBC opt-in; legacy call wrappers remain
- Retirement status: marker consumers remain intentionally deferred to Slice 2 compatibility adapter migration
- New risk signals:
- none
- Advisory decision: continue

## Final Checkpoint Update

- Current todo: complete
- Active slice: Slice 4 verified; developer guidance, architecture decision, retirement checks, and final acceptance evidence are complete
- Completed todos:
- Added the developer guide for error ownership, recovery, catalog extension, redaction, and verification
- Added the Agent v2 capability ADR and superseded the historical v3 error-contract signal
- Confirmed runtime `AGENT_PROTOCOL_VERSION` remains `2`
- Confirmed retired marker helpers have no business-module consumers; the marker remains only in the single compatibility adapter, explicit fixtures, and its static boundary test
- Re-ran Java, Rust, and frontend acceptance commands with current sources
- Evidence refs:
- `docs/backend-error-handling.md`
- `docs/aegis/adr/2026-08-01-structured-backend-error-contract-v2.md`
- `evidence-bundle-draft-slice4-final-acceptance.json`
- Blocked on: none
- Residual risk: real Dameng, DB2, and TDengine fault-injection environments were unavailable, so connection interruption, backpressure, timeout, cancellation, metadata retry, Runtime replacement, and legacy-Agent behavior were not revalidated against live databases
- Workspace governance note: the Aegis workspace check remains non-zero because the user-owned untracked index references one missing historical file and does not index several existing documents, including this task's new ADR; no unrelated index content was modified
- Next step: release through the documented capability rollout; run the live-database matrix before removing either the legacy marker adapter or synthesized multi-statement `Error` rows
