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

## DriftCheckDraft

- Scope status: Slice 1 stayed within Agent v2 and typed Rust corridor
- Compatibility status: protocolVersion remains 2; structured_error_v1 is explicit JDBC opt-in; legacy call wrappers remain
- Retirement status: marker consumers remain intentionally deferred to Slice 2 compatibility adapter migration
- New risk signals:
- none
- Advisory decision: continue
