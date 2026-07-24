# Nexus (Orvin) · Castor

> Local-first knowledge base — Obsidian-class vault on **Tauri 2 + Rust + React** with deep local LLM/RAG.  
> Brand **Orvin**, in-app AI companion **Castor**. Vault scale target 50k+ `.md`, i18n RU/EN, plugins (curated/demo path today).

**Status (2026-07):** engineering **feature-complete prototype** on `main` (RAG, second-brain jobs, news, board, agent/agentd, sandbox code default-OFF, PLUG-1 audit, design system).  
**Package version:** **`0.1.0`** (personal dogfood cut — not a public/signed release). Owner UI acceptance still has a large 🧪 queue; signed updater is not shipped.  
**Current product track:** personal dogfood (see `docs/BETA-SURFACE.md`) · start here: **`docs/GETTING-STARTED.md`**.  
**Progress log:** `CHANGELOG.md` · `[0.1.0]` cut 2026-07-24.

**Primary development agent:** Hermes (Igoryan) / Grok — not Claude Code. Project rules still load from `CLAUDE.md` (name is historical).

---

## What ships in claim vs what does not

| In **personal beta surface** (goal) | **Out of claim** until owner greenlight |
|---|---|
| Vault editor, graph, search, Home/Today | Untrusted plugin load / marketplace (PLUG-2) |
| Chat + RAG citations | Host shell / sandbox exec (`shell_enable`) |
| Castor embedded agent, confirm gate, undo | Remote agentd WS, multi-channel gateways |
| Board, news (optional), git-sync, backup | Signed auto-updater / notarization |
| Operator-curated demo plugin panel | Mobile apps, voice, cloud sync service |

Details: **`docs/BETA-SURFACE.md`**.

---

## Docs map (source of truth)

| Path | Role |
|---|---|
| `docs/GETTING-STARTED.md` | Dev + personal Mac unsigned run paths |
| `docs/BETA-SURFACE.md` | What we claim for personal dogfood; freeze rules |
| `docs/architecture/ARCHITECTURE.md` | Architecture + ADR journal §0 |
| `docs/acceptance/ACCEPTANCE.md` | Automated AC-… criteria |
| `docs/design/DESIGN.md` | UI contract |
| `docs/BACKLOG.md` | Deferred work registry (no silent caps) |
| `docs/THREAT_MODEL.md` | Threats & default-OFF matrix |
| `docs/CONFIGURATION-REFERENCE.md` | Config schema from code |
| `docs/AGENT-PROD-PLAN.md` | Agent-as-service plan (**historical + partial**; see superseded note inside beta surface) |
| `docs/NIGHT-PLAN.md` / `IMPROVEMENT_PLAN.md` | **Historical** autonomous queues — not the live roadmap |
| `CLAUDE.md` | Hard rules for any coding agent (ADR, anti-patterns, verify) |
| `CHANGELOG.md` | Per-slice engineering history |

**Owner UI acceptance** (“does it work in the live app?”) is **not** fully encoded in git: it lives in the operator STATUS journal (vault/archive). Automated green CI ≠ owner ✅.

---

## Quick start

Full dual-path guide (dev + personal Mac unsigned `.app`): **`docs/GETTING-STARTED.md`**.

**Needs:** Node ≥ 20 · pnpm ≥ 9 · Rust stable (`rustup`) + Tauri deps · OS webview (macOS WKWebView / Linux webkit2gtk — see CI).

```bash
pnpm install

pnpm dev                         # native Tauri window + HMR (real backend)
pnpm --filter @nexus/desktop dev # browser-only on :1420 — **mocks**, not acceptance

pnpm typecheck && pnpm lint && pnpm test
cd apps/desktop/src-tauri
source "$HOME/.cargo/env"   # if needed
cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Configure local LLM endpoints in `.nexus/local.json` (gitignored) — OpenAI-compatible chat + embedding. Homelab IPs in older notes are **examples**, not product defaults for strangers.

---

## ADR (short)

1. Plugins — **JS-first + host-broker** (not WASM-first).  
2. Security — **capability-broker + path-scoped permissions**; plugin code not in git.  
3. DB — **rusqlite + write-actor** (not sqlx).  
4. Graph SoT — **SQLite**.  
5. AI — split **Chat / Embedding**; cloud chat opt-in only.  

Agent-as-service (`nexus-agentd` + connector) is product reality; formal ADR-009 file under `docs/adr/` may still be pending — do not invent ADR numbers.

---

## Development discipline

Slice cycle: **implement → tests green → update docs → next**.  
Bugfix: regression test first. Deferred scope → `docs/BACKLOG.md`.  
Do not change ADR silently. Do not enable owner-gated security flags in real vaults without a written decision.

Agent kickoff details: `prompts/DEV-PROMPT.md` (method still valid; ignore “Claude Code only” wording where present).

---

## License / repo

Public GitHub: `Ikler33/nexus`. Package version **0.1.0** (unsigned personal cut; not notarized).
