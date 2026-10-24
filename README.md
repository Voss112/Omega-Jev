<div align="center">

# Ω Omega

**A local, anonymous AI coding agent. One native Rust binary. No server, no telemetry.**

[![Release](https://img.shields.io/github/v/release/Voss112/Anonymous?style=flat-square)](https://github.com/Voss112/Anonymous/releases/latest)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-blue?style=flat-square)
![Rust](https://img.shields.io/badge/rust-2024-orange?style=flat-square)

[**Download the MSI**](https://github.com/Voss112/Anonymous/releases/latest) · [Français](README.fr.md)

</div>

---

## Why Omega

Omega splits the work between two systems:

- **System One — Jev (TypeSafe)** makes every control decision: skill, reasoning budget, web access, risk, impact.
- **System Two — DeepSeek V4.1 Flash, Claude or Gemini** writes the code and runs the tools.

Pick the provider, the exact model and the effort level right in the input box. The model actually used stays visible in the header, on every reply and in the Tools panel.

## Highlights

- **Fully local** — data in `%APPDATA%/Omega`, API keys encrypted with Windows DPAPI.
- **Private by design** — generic User-Agent, anti-SSRF, optional Tor/SOCKS5 proxy, VPN detection and kill-switch.
- **Local voice dictation** — Whisper large-v3-turbo on your GPU (Vulkan, CPU fallback). Nothing is recorded or uploaded.
- **Safe edits** — every file is backed up before the agent writes it, one-click restore (↶), coloured diffs, automatic `cargo check` / `tsc` / `go vet` / `py_compile`.
- **Token savings you can measure** — lazy tools, output compression, prompt caching and compaction, with real provider numbers.
- **Built-in security audit** — 25 CWE rules in `security_scan` plus the Cloudflare `security-audit` skill.
- **Subscriptions or API keys** — Claude Pro/Max, Google AI Pro/Ultra through the **Antigravity CLI** (`agy`, text only — Google closed Gemini Code Assist to personal accounts in June 2026; Code Assist remains for Google Cloud projects), or plain keys.

## Install

### Windows (recommended)

1. Download **`Omega-0.9.3-x64.msi`** from the [latest release](https://github.com/Voss112/Anonymous/releases/latest).
2. Double-click it. The install is per-user: no admin rights needed, and a Desktop shortcut is created.
3. On first launch, **⚙ Settings** opens. Paste your **DeepSeek** key ([platform.deepseek.com](https://platform.deepseek.com)) and your **Jev** key ([console.typesafe.ai](https://console.typesafe.ai)). Without a Jev key, a local router takes over.
4. Claude and Gemini connect through the **Sign in** button (subscription) or an API key.
5. Pick a working folder (📁, bottom left). Default: `Documents\Omega`.

Keys can also come from environment variables: `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `TYPESAFE_API_KEY`.

> Voice dictation uses `vulkan-1.dll`, which ships with every GPU driver.

### Linux

Build from source (below). The usual X11/Wayland packages are required at runtime, as for any egui app.

## Features

### Interface

| | |
|---|---|
| **Provider · model · effort** | ⟳ fetches the real model list of your account. "✎ Other id…" accepts any identifier. Provider errors (404, missing model) are shown as is: no silent fallback. |
| **Files** (📁) | Workspace tree, syntax-highlighted tabs, line numbers, **✎ Edit** + `Ctrl+S` (CRLF/LF preserved). Unsaved tabs are never closed or overwritten. |
| **Tasks** (`/tasks`) | Upcoming, paused and finished jobs with countdown, run history, Run now, Pause/Resume, Edit, Delete. |
| **⏰ Schedule** | Prompt + day + time + engine + model + effort, optional daily repeat. Runs in bypass mode in its own chat, without touching your settings or draft. |
| **🎙 Dictation** (`Ctrl+M`) | Each sentence lands in the input box at the pause. `Enter` stops and sends, `Esc` stops. |
| **Images** | Drag and drop, `Ctrl+V` or 🖼. Resized to 1568 px, 4 per message. |
| **Tables** | Columns sized to the reading width, wrapped cell text. |
| **`/clear`** | New context. The old chat stays in history. |
| **Export** | ⇩ any chat to Markdown. ✏ edit and ⟳ regenerate are available (files restored first). |

### Delegation and cost control

- **Delegation** — simple questions, summaries, **unit / integration / end-to-end tests** and **comments** go to **Gemini Flash**, then **DeepSeek V4.1 Flash** if Gemini is unavailable or out of quota, then your main engine. Gemini through Antigravity is text only: it takes simple questions and summaries, while tests and comments (tool work) go to DeepSeek Flash under Omega's shell guard.
- **Auto model (Claude)** — Jev rates each turn: simple edits → **Haiku**, everyday work → **Sonnet**, hard tasks → your model. Never above your choice, never at the expense of a warm cache.
- **Continuous tests** — tests run in the background while the agent codes (`cargo watch`, `vitest`, `jest`, `ptw`, `gotestsum`, `dotnet watch test`, `gradle --continuous`, or Omega's own watcher).
- **Savings proof** (⚙ Settings) — per-request input, cache read/write, output and Jev usage, plus a benchmark of Jev on 7 labelled prompts.
- **Unlimited steps** — the **Infinite** switch allows long unattended tasks. ■ Stop always works.

### Built-in tools

| Replaces | Omega implementation |
|---|---|
| lazy-tool | Jev picks the skill and only its tools are sent. The set grows monotonically per chat, so the cache stays stable. |
| Serena | `outline`, `find_symbol`, `replace_symbol` through tree-sitter (11 languages), syntax-checked edits. |
| code-review-graph | `blast_radius`: petgraph reference graph, reverse BFS to depth 3. |
| RTK | Shell output compression (ANSI, noise, passed tests, duplicates) with cargo, git, pytest and npm filters. |
| TOON | Tab-separated tabular results, grouped and parallel `grep`. |
| caveman | 4 output styles, no narration between tools, grouped calls. |
| Superpowers | 15 official playbooks (MIT), at most one injected per chat. |
| Cloudflare audit | `security-audit` skill embedded verbatim (MIT) with `audit_kit`. |
| Semgrep / mcp-scan | `security_scan`: 25 CWE rules, masked secrets, deterministic shell guard + Jev review + approval. |
| CLI-Anything | `cli_anything`: 100+ CLIs (GIMP, Blender, LibreOffice, OBS…), install validated by you, `--json` execution without a shell. |
| Web search | Anonymous DuckDuckGo, Jev Score ranking, dense Markdown pages, local BM25 cache. |

## Security and privacy

- **Reversible edits** — backups before every write, coloured diffs, optional per-edit approval.
- **Bypass mode** (🔒/🔓) — accepts all permissions; destructive commands stay blocked.
- **Clean process kill** — a timed-out or stopped command kills its whole process tree.
- **Secrets stay secret** — API keys are DPAPI-encrypted, stripped from the environment of every process the agent starts, and never written in clear text (a DPAPI failure refuses the write).
- **Subscriptions** — OAuth tokens sealed in `credentials/` (DPAPI).
- **Voice** — captured and transcribed on your machine. The microphone is open only while dictating.
- **Network** — private-address redirects and DNS resolutions are refused (anti-SSRF), web pages are marked untrusted, optional Tor/SOCKS5 proxy (`socks5h://127.0.0.1:9050`), VPN detection, optional kill-switch. The exit-IP check (ipwho.is) is **off by default**.
- **Local data only** — `%APPDATA%/Omega` on Windows, `~/.local/share/Omega` on Linux (files `0600`).

## Build from source

```sh
cargo build --release                      # optimised binary (LTO), embedded icon
cargo test                                 # tools, domain, DPAPI, SSRF, checkpoints
powershell -File installer/build-msi.ps1   # MSI (WiX 3.x in %USERPROFILE%\tools\wix314 or $env:WIX_BIN)
```

Windows target `x86_64-pc-windows-gnu`: dictation compiles whisper.cpp. Requirements (paths in `.cargo/config.toml`): MinGW-w64 (gcc, CMake, Ninja), `libclang.dll` for bindgen, and Vulkan headers + `glslc` + `libvulkan-1.dll.a` (MSYS2 ucrt64 packages). `.cargo/config.toml` holds machine-specific paths: adapt them to your setup.

## Architecture

```
src/
  domain/   pure model: chat, route, skills, model catalogue, tools, settings
  infra/    engine clients (SSE), Jev, disk, HTTP, processes, VPN, DPAPI, voice
  app/      use cases: Jev router, context assembly, agent loop, dictation, self-tests
  tools/    native tools (fs, code, graph, grep, shell, security, web, compression)
  ui/       egui interface (theme, sidebar, explorer, chat, composer, scheduler, tools, settings)
```

## Project rules

Drop an `OMEGA.md` at the root of a project to give the agent permanent rules (button "Create OMEGA.md" in the Tools panel). `AGENTS.md` is read too. `CLAUDE.md` is **never** sent: it is written for Claude Code and would be re-billed on every request.
