# Getting started — personal dogfood (Orvin / Nexus)

**Version:** 0.1.0  
**Audience:** owner (Artem) + agent building from source  
**Not:** public install guide, signed DMG, auto-updater

Product claim boundary: **`docs/BETA-SURFACE.md`**.  
Config reference: **`docs/CONFIGURATION-REFERENCE.md`**.

---

## Two ways to run

| Path | When | Acceptance? |
|---|---|---|
| **A. Dev (recommended for M-β1)** | Daily dogfood + debugging | **Yes** — native Tauri window |
| **B. Unsigned `.app` (Mac)** | “App-like” launch without `pnpm dev` | **Yes** if built from same `main` / tag |
| Browser `:1420` only | Frontend HMR / mock UI | **No** — mocks, not shipped backend |

---

## A. Development (any OS with Tauri deps)

**Needs:** Node ≥ 20 · pnpm (see root `packageManager`) · Rust stable + Tauri system deps · OS webview.

```bash
git clone git@github.com:Ikler33/nexus.git && cd nexus
# or: existing clone on the dogfood machine
pnpm install
pnpm dev    # native Orvin window + HMR + real Rust backend
```

Browser-only (not for acceptance):

```bash
pnpm --filter @nexus/desktop dev   # http://localhost:1420 — mocks
```

Checks agents use before merge:

```bash
node scripts/check-versions.mjs
pnpm typecheck && pnpm lint && pnpm test
cd apps/desktop/src-tauri
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Full local gate (slow): `bash scripts/test-all.sh`.

---

## B. Personal unsigned Mac `.app` (owner machine)

Build **on macOS** (this is not done on Hermes LXC):

```bash
pnpm install
pnpm build:app
# equivalent: pnpm --filter @nexus/desktop app:build
```

Artifact (typical Tauri 2 layout):

```text
apps/desktop/src-tauri/target/release/bundle/macos/Orvin.app
```

Open:

1. Finder → right-click **Orvin.app** → **Open** (first launch may need Gatekeeper override).  
2. Or: `open apps/desktop/src-tauri/target/release/bundle/macos/Orvin.app`  
3. If blocked: System Settings → Privacy & Security → allow Orvin; or  
   `xattr -dr com.apple.quarantine Orvin.app` (only for **your** build).

**Not shipped:** Apple notarization, Developer ID signature, Sparkle/Tauri auto-updater.  
Do not distribute this binary as “release”; it is a personal dogfood bundle.

Version banner / `app_version` should show **0.1.0** (+ git branch/hash when built from a dirty/dev tree per build-info).

---

## LLM / vault first run

1. Open or create a vault (local folder of `.md`).  
2. Configure OpenAI-compatible **chat** + **embedding** endpoints in vault  
   `.nexus/local.json` (gitignored) — schema: `docs/CONFIGURATION-REFERENCE.md`.  
3. Settings → AI: change URL and re-check; errors must be honest (no infinite spinner).  
   Chat empty state shows **Configure AI** when chat URL is unset; missing provider → banner, not silent hang.  
4. Homelab IPs in old notes are **examples**, not product defaults.

Dangerous flags (`shell_enable`, sandbox host exec, untrusted plugins) stay **default-OFF**.  
Do not enable without a written security decision.

---

## Acceptance pack (owner)

Live **Tauri** window only — checklist in `docs/BETA-SURFACE.md` § Acceptance pack  
(and multi-lens synthesis §6 if you have the NAS review pack).

CI green ≠ owner ✅.

---

## Out of this guide

- PLUG-2 / marketplace  
- Remote `nexus-agentd` as default UX  
- Signed updater / notarization  
- Mobile  

---

*M-β3 · 2026-07-24*
