# SDD ledger — plan: docs/superpowers/plans/2026-10-23-abonnements-claude-gemini.md

Repo has no lib target: `cargo test --lib` cannot work; use `cargo test` (bin tests).
Task 1: Ruling: plan says `cargo test --lib` — crate is bin-only, so every verification uses `cargo test [filter]` — costs nothing, same coverage.
Task 0: complete (git init, baseline commit 7062c5a).
Task 1: complete (Engine x4, Wire, Effort, Efforts; tests domain::engine 3/3 + infra::llm 3/3, `cargo test` 13 passed).
Task 2-3-4-5: Ruling: modules claude/gemini/pkce are declared by `infra/auth/mod.rs`, so a per-task commit would not compile — shipped as one commit `00ce160` spanning tasks 2–5 — cost: coarser history, same code.
Task 2-5: complete (creds sealed DPAPI, stale(), PKCE RFC7636 vector, claude authorize/exchange/refresh/import/usage, gemini authorize/exchange/refresh/tier/envelope; `cargo test auth` 13/13, full suite 26 passed).
Task 6-7: complete (llm split into openai/anthropic/gemini + facade; 36 tests green; commit 39030f3).
Task 6-7: Ruling: `Completion` gained a `#[doc(hidden)] blocks` map (Anthropic block index → tool position) so `input_json_delta` events can be routed without global state — cost: one extra field in a public struct, no API change for callers.
Task 8: complete (Env unchanged, health probes Claude+Gemini with mode labels, agent message covers subscription; clippy -D warnings clean).
Task 9: complete (engine_label/effort_label, two pickers, plan-usage panel section; test ui::composer 1/1).
Task 10: complete BUT two deviations — see rulings.
Task 10: Ruling: OAuth exchange signatures take `(verifier, state/redirect)` strings instead of `&Pkce`, because the verifier must survive in UI state after `Pkce` is dropped — cost: none, clearer.
Task 10: Ruling: `Login` lives in `app::login` (data) and is rendered by `ui::login`, not `ui::login::Login` as the plan wrote — cost: one import path differs from the plan.
Task 10: complete (claude_start/gemini_start/finish_paste/await_gemini/import_cli/persist, modal, settings section, Event::Connected/PlanUsage; `cargo test` 39 passed, clippy clean, `cargo build` OK; commit 82e9f26).
Plan 2 (2026-10-23-skill-audit-securite.md): Task 1 complete (bundle c1c8a8c vendored verbatim + SOURCE.md, Group enum, 16 playbooks; test playbook 2/2, commit 0f3d3c0).
Plan 2 Task 2: complete (Skill::Security -> playbook security-audit, [audit] hint in preamble, tests context 2/2).
Plan 2 Task 3: complete (tools/audit.rs bundle macro, install idempotent, never overwrites findings.json, ToolKind::AuditKit 16e outil, schema; tests tools::audit 3/3).
Plan 2 Task 4: complete (health entry "Skill audit (Cloudflare)" + playbooks group label + README; clippy -D warnings clean, 48 tests green).
<!-- end -->
