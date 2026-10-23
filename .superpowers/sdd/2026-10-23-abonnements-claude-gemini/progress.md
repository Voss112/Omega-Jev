# SDD ledger — plan: docs/superpowers/plans/2026-10-23-abonnements-claude-gemini.md

Repo has no lib target: `cargo test --lib` cannot work; use `cargo test` (bin tests).
Task 1: Ruling: plan says `cargo test --lib` — crate is bin-only, so every verification uses `cargo test [filter]` — costs nothing, same coverage.
Task 0: complete (git init, baseline commit 7062c5a).
Task 1: complete (Engine x4, Wire, Effort, Efforts; tests domain::engine 3/3 + infra::llm 3/3, `cargo test` 13 passed).
Task 2-3-4-5: Ruling: modules claude/gemini/pkce are declared by `infra/auth/mod.rs`, so a per-task commit would not compile — shipped as one commit `00ce160` spanning tasks 2–5 — cost: coarser history, same code.
Task 2-5: complete (creds sealed DPAPI, stale(), PKCE RFC7636 vector, claude authorize/exchange/refresh/import/usage, gemini authorize/exchange/refresh/tier/envelope; `cargo test auth` 13/13, full suite 26 passed).
<!-- end -->
