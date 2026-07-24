# Beta surface — personal dogfood (Orvin / Nexus)

**Date:** 2026-07-23  
**Track:** M-β0…M-β3 (personal daily driver for the owner)  
**Not:** closed public beta, signed release, plugin marketplace  

This document is the **product claim boundary**. Engineering may have more code than we claim.  
If it is not listed under **In surface**, do not advertise it as “works” for dogfood.

---

## Goal

Artem can use Orvin **daily** on his Mac with a real vault + local/LAN OpenAI-compatible LLM:

- notes without data-loss surprises  
- search + RAG chat with sources  
- Castor tasks with **confirm gate** + undo  
- Home/Today usable  
- backup export/import  

Success metric (default until owner overrides): **≥5 open-days/week** and **0 lost notes**.

---

## In surface (we aim to prove with owner ✅)

| Area | Notes |
|---|---|
| Vault CRUD | create/edit/autosave/rename/delete/trash |
| Editor + reading view | known polish debt OK if no P0 bugs |
| Command palette / search / recents | M11-class |
| Graph open | no crash; isolation toggle OK |
| Chat RAG | sources, stop, overflow menu |
| Castor **embedded** | plan, changeset+diff, approve, undo, multiturn gate |
| Board | DnD + hide done (already owner-accepted) |
| News | optional; not required for daily claim |
| Backup | export/import |
| Settings | theme, i18n, AI endpoints with honest errors |
| Dev self-check / version banner | when running dev builds |

---

## Out of surface (do not claim)

| Area | Why |
|---|---|
| Untrusted plugin execution / marketplace | PLUG-2 blocked (nav-egress); demo iframe only |
| `shell_enable` / sandbox host exec | owner-gated; built ≠ safe |
| Remote agentd (WS/TLS), multi-tenant | attack surface |
| ACP live vs Hermes as default UX | protocol exists; not beta narrative |
| Signed DMG / auto-updater / notarization | prod track |
| Mobile | post desktop |
| Voice / email / multi-channel gateways | post-1.0 |
| Cloud sync service | violates local-first story (git-sync is enough) |
| True CM6 WYSIWYG live-edit / Dataview | large epics |
| Autonomy `auto` actuator without sandbox | threat model |

---

## Feature freeze (M-β0)

Until M-β1 acceptance pack is closed (or owner lifts freeze):

**Allowed**
- P0 bugs on in-surface paths (esp. M2 recurring insights/contradictions)  
- Docs / SoT / VERSION process / CI honesty  
- Tests that lock current behavior  
- Personal 0.1.0 packaging prep  

**Not allowed**
- New epics (MCP productization, PLUG-2 load, remote agent, reskins, mobile)  
- Expanding default-ON dangerous flags  
- Quiet scope growth without BACKLOG line  

---

## Acceptance pack (owner — live Tauri window)

See also multi-lens synthesis §6. Short list:

1. Cold start + vault + LLM self-check honesty  
2. Note loop + split close  
3. Find / palette  
4. Chat RAG + menu + stop  
5. Castor approve/undo + slow gate + multiturn  
6. Today/Home; if insights ON — **still works after app restart** (M2)  
7. Board smoke  
8. Backup round-trip  
9. Settings endpoint change  
10. LLM down → clear error, not infinite spinner  

Resolutions go only in **owner STATUS** journal (archive/vault). CI green is not a substitute.

---

## Roadmap names (Hermes)

| Milestone | Intent |
|---|---|
| **M-β0** Truth & Freeze | this doc + README + SoT — **done** |
| **M-β1** Acceptance sprint | owner fills ✅ (live Tauri) |
| **M-β2** Silent brain fix | M2 recurring + scheduler audit — **done** |
| **M-β3** Personal 0.1.0 | version + changelog cut + GETTING-STARTED — eng **done**; Mac unsigned `.app` — owner |
| **M-β4** Hardening | daily-loop polish after dogfood |

---

## Superseded / historical docs (do not treat as live queue)

| Doc | Treat as |
|---|---|
| `docs/NIGHT-PLAN.md` | Historical autonomous night queue |
| `docs/IMPROVEMENT_PLAN.md` | Historical “second brain” execution log |
| `docs/AGENT-PROD-PLAN.md` | Architecture intent + partial status; **re-read code/CHANGELOG** before trusting “next slice” lines |
| `prompts/DEV-PROMPT.md` “Phase 0 only” framing | Method OK; phase framing stale |
| Old README “Фаза 0 в работе” | Replaced 2026-07-23 |

Live open work: `docs/BACKLOG.md` + this surface + Hermes kanban board `nexus`.  
User-facing start: **`docs/GETTING-STARTED.md`**.

---

## Defaults locked for this track (2026-07-23)

Unless owner overrides in chat:

- Goal = **personal dogfood only** (not friend closed beta first)  
- **M2 fix = yes** (reopen June deferral)  
- Out-of-claim list above = **yes**  
- Git: **branch + PR**, merge only on owner «го»  
- PLUG-2 = **not before explicit security ceremony**  
- Primary coding agent = **Hermes/Grok**  

---

*Owned by product track · update when surface changes*
