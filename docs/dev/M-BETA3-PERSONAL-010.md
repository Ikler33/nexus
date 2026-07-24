# M-β3 Personal 0.1.0 — notes

**Branch:** `release/m-beta3-0.1.0`  
**Date:** 2026-07-24  
**Agent:** Hermes / Igoryan  

## Done in this milestone (eng / docs)

- [x] Version **0.0.0 → 0.1.0** in 4 SoT (`check-versions.mjs`)  
- [x] CHANGELOG cut: `## [0.1.0] - 2026-07-24` + empty `[Unreleased]`  
- [x] `docs/GETTING-STARTED.md` dual path (dev + Mac unsigned)  
- [x] README / BETA-SURFACE / BACKLOG honesty  
- [ ] Mac unsigned `.app` live build — **owner**  
- [ ] Optional git tag `v0.1.0` — after owner OK / dogfood  

## Out of scope

- Notarization / auto-updater  
- M-β1 owner acceptance resolutions  
- New product epics  

## Verify

- `node scripts/check-versions.mjs` → `0.1.0`  
- Frontend/unit paths that care about version  
- Full CI matrix on merge (ubuntu + mac + windows rust)  
- Mac: `pnpm build:app` → Orvin.app opens (owner)  

## PARTIAL without Mac

Eng packaging + docs = done when CI green.  
Personal “daily without is this main?” anxiety closes only after owner runs path B once.
