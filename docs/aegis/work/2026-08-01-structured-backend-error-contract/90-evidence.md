# Structured backend error contract implementation - Evidence

## EvidenceBundleDraft

- Artifact key: slice1-java-agent-contract
- Type: test
- Source: .\gradlew.bat :common:test --tests *Agent*
- Summary: Java Agent protocol and structured error producer tests completed successfully
- Verifier: Gradle exit code 0

## EvidenceBundleDraft

- Artifact key: slice1-rust-agent-corridor
- Type: test
- Source: cargo test -p dbx-core --no-default-features db::agent_driver::tests
- Summary: 49 Agent driver tests passed, including strict v1, legacy, timeout, cancel and runtime failure cases
- Verifier: Cargo exit code 0

## EvidenceBundleDraft

- Artifact key: slice1-static-checks
- Type: verification
- Source: cargo fmt --all -- --check; cargo check -p dbx-core --no-default-features; JSON fixture parse; git diff --check
- Summary: Formatting, compilation, fixture syntax and whitespace checks passed
- Verifier: All commands exit code 0

## EvidenceBundleDraft

- Artifact key: slice4-final-acceptance
- Type: verification
- Source: fresh Java/Rust/frontend acceptance matrix, static retirement search, developer guide, and Agent v2 ADR
- Summary: Agent tests succeeded; dbx-core targeted suites passed (agent driver 51, query 90, schema 78, connection 288 with 4 ignored, catalog 5, recovery 1); dbx-web error tests passed 3; Rust formatting and three crate checks passed; frontend typecheck and 196 focused tests passed. Runtime protocol remains v2 and business modules no longer consume legacy marker helpers.
- Verifier: every executed command returned exit code 0; static search results were manually classified
- Residual risk: no live Dameng, DB2, or TDengine fault-injection regression was available

## EvidenceBundleDraft

- Artifact key: slice3-transport-and-frontend
- Type: test
- Source: core/web serialization tests; `cargo check` for dbx-core/dbx-web/dbx; `pnpm typecheck`; focused Vitest suites
- Summary: Tauri and HTTP preserve BackendError objects, multi-result/progress errors remain structured, all catalog keys exist in eight locales, and queryStore compatibility behavior passed
- Verifier: Rust/TypeScript commands exit code 0; 196 focused frontend tests passed

## EvidenceBundleDraft

- Artifact key: slice2-core-recovery-and-catalog
- Type: test
- Source: `cargo test -j 1 -p dbx-core --no-default-features --lib query::tests`; `schema::tests`; `connection::tests`; `db::agent_driver::tests`; `backend_error::tests`; `agent_recovery::tests`
- Summary: Typed Agent recovery decisions, metadata retry/quarantine behavior, keepalive session routing, catalog mappings, redaction, and marker boundary tests passed (90 + 78 + 288/4 ignored + 51 + 5 + 1)
- Verifier: Cargo exit code 0 for every command

## EvidenceBundleDraft

- Artifact key: slice2-core-static-checks
- Type: verification
- Source: `cargo fmt --all -- --check`; `cargo check -j 1 -p dbx-core --no-default-features`; `rg` retired marker consumers
- Summary: Formatting and compilation passed; legacy parser names remain only in the compatibility adapter test boundary and explicit fixtures
- Verifier: All commands exit code 0
