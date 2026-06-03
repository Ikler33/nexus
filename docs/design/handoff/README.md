# Handoff: Nexus — local-first PKM editor with an AI layer

## Overview
**Nexus** is a desktop, local-first knowledge/notes app (Obsidian-class) with a built-in
AI/RAG layer. It is keyboard-first, dense but calm, and ships in **light** ("old paper")
and **dark** ("warm clay") themes, with **RU/EN** UI. Target shell is **Tauri** (Rust +
WebView), so the UI is web tech but must feel like a native desktop app.

This bundle covers the full product surface:
- **Main workspace** — titlebar, sidebar (file tree / search / tags / starred), tabbed
  editor with Markdown + `[[wikilinks]]` + `#tags` + backlinks, two-pane split, AI panel
  (chat/RAG, suggestions, summary), command palette, graph view, reading mode.
- **Home dashboard** — a landing/overview screen (greeting, search, quick actions, and a
  grid of widgets: daily brief, recent, goals, stats, stale radar, open questions, focus drift).
- **Onboarding**, **Conflict resolver** (3-way merge), **Plugin manager + permission consent**.

## About the Design Files
The files in this bundle are **design references created in HTML/CSS/React-via-Babel** —
prototypes that show the intended look and behavior. They are **not** production code to copy
verbatim. The task is to **recreate these designs in the target codebase's environment**
(for Nexus that is a Tauri app — pick a front-end framework, React is the natural fit since the
prototype is already React) using its established patterns, build tooling, and component
libraries. The prototype uses in-browser Babel + global `window.*` components purely so it can
run from static files; a real implementation should use proper modules/bundling.

The design system is **token-driven** (`tokens.css`) — every color/space/radius/font is a CSS
variable, themes swap by `data-theme`, accent swaps by `data-accent`. Port the token layer first;
everything else consumes it.

## Fidelity
**High-fidelity.** Final colors, typography, spacing, motion, and interaction states are all
specified here and in the CSS. Recreate pixel-faithfully, but use the codebase's real building
blocks (a real CodeMirror/ProseMirror editor instead of the mock textarea/markdown renderer, a
real force-graph lib or the included sim, real i18n, etc.).

---

## Design Tokens (source of truth: `tokens.css`)

### Hue system
- `--ui-hue` — the single warm hue the whole neutral palette is built on.
  Light theme overrides it to **85** (yellow "old paper" parchment); base/dark use **50** (warm clay).
- `--acc-h` — accent hue, default **47** (terracotta). Selection tints derive from the accent hue.

### Color tokens (semantic; values change per theme — see `tokens.css` for both themes)
| Token | Light (paper) | Dark (clay) | Use |
|---|---|---|---|
| `--color-bg` | `oklch(0.966 0.013 85)` | `oklch(0.205 0.010 50)` | editor canvas |
| `--color-chrome` | `oklch(0.910 0.018 85)` | `oklch(0.232 0.012 50)` | titlebar / sidebar / tab strip / status bar |
| `--color-bg-elevated` | `oklch(0.996 0.005 85)` | `oklch(0.258 0.013 50)` | cards, popovers, menus |
| `--color-surface` | `oklch(0.933 0.015 85)` | `oklch(0.262 0.013 50)` | inputs, sunken chips |
| `--color-surface-hover` | `oklch(0.893 0.020 85)` | `oklch(0.300 0.015 50)` | hover |
| `--color-selected` | `oklch(0.875 0.018 80)` | `oklch(0.32 0.022 47)` | **unified** selected-row / active-tool bg |
| `--color-text` | `oklch(0.30 0.014 85)` | `oklch(0.905 0.012 50)` | primary text |
| `--color-text-muted` | `oklch(0.47 0.015 85)` | `oklch(0.705 0.014 50)` | secondary |
| `--color-text-faint` | `oklch(0.62 0.013 85)` | `oklch(0.525 0.013 50)` | tertiary / icons at rest |
| `--color-border` | `oklch(0.86 0.014 85 / …)` | `oklch(0.36 0.010 50 / …)` | hairlines (chrome-gated) |
| `--color-accent` | `oklch(0.605 0.135 47)` | lighter (+0.10 L) | terracotta — primary actions, active icon, indicators |
| `--color-ai` | `oklch(0.56 0.066 205)` teal | `oklch(0.72 0.072 200)` | AI/info layer |
| `--color-link` | deep teal-blue | lighter | `[[wikilinks]]` |
| `--color-tag` | sage | lighter | `#tags` |
| `--color-success / warning / danger` | sage / amber / red-clay | brighter | status |

Accent is swappable via `data-accent` ∈ `amber`(default) `teal` `sage` `clay` — each sets
`--acc-l/--acc-c/--acc-h`. **Selection color is intentionally unified**: tree rows, active rail
buttons, and active titlebar buttons all use `--color-selected` (a deep warm taupe), with the
accent reserved for the icon + a 3px left indicator bar — do not fill rows with saturated accent.

### Type
- UI: **Onest** (400/500/600/700) — geometric grotesk w/ Cyrillic.
- Mono/meta: **JetBrains Mono** (400/500/600).
- Editorial prose (greeting, AI prose, open questions, big stat numbers): **Source Serif 4**.
- Scale: `--text-xs 11 · sm 12 · base 13 · md 14 · lg 16 · xl 20 · 2xl 26`. Editor body 16px, measure 760px fixed.

### Spacing / geometry / motion
- Space grid (×4): `--space-1..8 = 4,8,12,16,20,24,32,48`. `--row-h` 28px (24 compact), gated by `--density`.
- Radii: `--radius-sm 4 · md 7 · lg 11`; pills `99px`.
- Elevation: `--elevation-1` (subtle) / `--elevation-2` (popovers); `--chrome-shadow`, `--tab-shadow`.
- Motion: `--motion-fast 120ms · base 200ms`; eases `--ease-standard`, `--ease-spring`(overshoot),
  `--ease-out`, `--ease-inout`. Durations `--dur-1..4 = 130/220/320/440`. All decorative motion is
  gated behind `prefers-reduced-motion`. See `motion.css`.

---

## Screens / Views

### 1. Main workspace (`Nexus.html` → `app.jsx`)
**Layout:** CSS grid `38px titlebar / 1fr body / 26px status`. Body is grid
`sidebar(resizable 180–420) | editor-split | [ai-panel(resizable 280–560)]`. Reading mode hides
chrome and centers the doc.

**Titlebar** (`.titlebar`, Liquid-Glass: `backdrop-filter: blur(16px)` over `--color-chrome`):
traffic lights (mac left / win right via `data` toggle), sidebar toggle, **brand mark**
(constellation logo) + "Nexus", centered pill **global search** (`⌘K`), right tool group: graph
(`⌘⇧G`), `RU / EN` text toggle (active side accent), theme (sun/moon, animated cross-fade), AI panel.

**Sidebar** (`.sidebar` on `--color-chrome`): icon **rail** (files/search/tags/starred; active =
`--color-selected` bg + accent icon) then the active panel. File tree rows (`.tree-row`): twist
chevron, file/folder icon, name (ellipsis), optional ★. Active row = `--color-selected` + 3px accent
left bar + accent icon + 500 weight.

**Editor** (`editor.jsx`): tab strip on chrome with rounded "floating" tabs (active lifts with
`--tab-shadow` + 2px accent top bar), `+`, split button. Tabs are **draggable between panes**
(`dataTransfer "text/nexus-tab"`). A single **floating Edit/Preview toggle** sits top-right of the
note (`.mode-float`, pill, semi-transparent → opaque on hover; icon shows the *action*: pencil in
preview, book in edit; `⌘E`). Preview renders Markdown (h1–3, lists, quote, code, **bold**,
`[[wikilink]]`→`--color-link`, `#tag`→pill). Edit shows raw Markdown in a mono textarea; both modes
share identical measure/padding so text doesn't shift. Backlinks bar pinned at bottom.

**AI panel** (`ai-panel.jsx`): header (sparkles + "AI-ассистент" + provider pill local/☁cloud),
tabs Chat/Suggestions(Links)/Summary. Chat: empty state (animated constellation mark + suggestion
pills) → on send a **"thinking" animated brand mark** (~1.3s) → **smooth char-by-char streaming**
(RAF, ~62 cps, blinking caret, Stop button) → answer with **RAG sources** (3 styles: cards/chips/
footnotes) + provider. Offline & cloud banners. Composer: single rounded box, soft accent focus
ring, send/stop. AI panel layout is tweakable: side / bottom / overlay.

**Command palette** (`palette.jsx`, `⌘K`): glass modal, files + commands, ↑↓ + Enter, staggered
item reveal; styles top/center/spotlight.

**Graph view** (`graph.jsx` + `graph.css`): force-directed graph of wikilinks. **Persistent,
re-heatable sim** — dragging a node pins it to the cursor and neighbours follow via springs; grabbed
node glows/scales, its edges light up. The **currently-open note** pulses (haloed ring + one-shot
ripple) and its edges are solid **pulsing** accent lines (`.g-edge.flow`, staggered), neighbours get
an accent ring. Loading uses the animated brand mark (not a spinner). Opens beside the note (resizable)
or fullscreen.

**Reading mode** (`⌘R`) and **adaptive density** (compact/comfortable/auto-by-width) are in Tweaks.

### 2. Home dashboard (`Nexus Home.html` + `home.css`)  ← the new screen
A landing/overview view that reuses the **same chrome** (titlebar + sidebar + status bar) with a
new sidebar nav section at top (`Home` active, `Новая заметка`). Main = scrollable dashboard,
`max-width 1080px`, centered, `padding 32px`.

**Sections (top→bottom):**
1. **Greeting header** — serif `30px` "Добрый день, *Артём*" (name in accent italic), sub-line
   (date · note count · changes today), meta chips: live provider chip (`ollama · qwen3:35b`, green
   glow dot), `vault: ~/notes`, `серия: 23 дня`.
2. **Hero search** — large elevated rounded input "Поиск по базе или вопрос к ассистенту…",
   `/` and `?` kbd hints; hover = accent focus ring.
3. **Quick actions** — pill buttons (`.qa`) with accent icons: Новая заметка, Daily note, Быстрая
   мысль, Граф, Переиндексировать.
4. **Section label** pattern: tiny uppercase faint text + trailing hairline (`.sec-label`).
5. **сводка** grid-2: **Daily brief** (AI card) + **Recent files** list.
6. **проекты** grid-2: **Goals progress** (3 bars: accent/ai/success) + **Stats** (2×2, serif numbers).
7. **требует внимания** grid-2: **Stale radar** (hot=danger / warm=warning glow dots, hover reveals
   action) + **Open questions** (AI, serif italic, left-rule, hover→ai).
8. **анализ** full-width: **Focus drift** (AI, serif prose, `em`→warning).

**Card anatomy** (`.h-card`): `--color-bg-elevated`, 1px border, `--radius-lg`, 18px pad, hover
lifts with `--elevation-1`. **AI cards** (`.h-card.ai`) get an `--color-ai`-tinted border + a 2px
teal left bar + an **`AI` badge** next to the title (teal). Card actions ("обновить", "все →") are
faint, accent on hover. The AI widgets are the ones whose content is model-generated — keep them
visually distinct from static widgets via the teal treatment.

Grids collapse to single column under 920px.

### 3. Onboarding (`Onboarding.html` + `onboarding.*`)
Welcome → choose vault → LLM server check (local/cloud, "unavailable" state) → first indexing with
progress → enter. Same tokens/motion; small server-status spinner is intentional (not the brand mark).

### 4. Conflict resolver (`conflict.html`-equivalent in `conflict.jsx/.css`)
3-way merge: local vs remote vs base, per-hunk accept, result preview.

### 5. Plugin manager + permission consent (`plugins.jsx/.css`)
Modal with left nav (Installed/Marketplace + privacy note), plugin cards (toggle switch, version,
author on one line, color-coded **permission chips**: red=sensitive net/shell, amber=caution
write/clipboard, neutral=read). Toggling on / installing a plugin with non-safe perms opens a
**consent sheet** (per-permission rows with risk badges + human descriptions, Allow/Cancel,
revocable note). Safe (read-only) plugins skip the sheet.

---

## Interactions & Behavior
- **Theme toggle**: adds `.theme-anim` to `<html>` for a 320ms cross-fade, swaps `data-theme`,
  persists to `localStorage("nexus-theme")`. Lang persists to `localStorage("nexus-lang")`.
- **Streaming chat**: time-based RAF reveal (≈62 chars/s) so it's smooth, not chunky; Stop cancels
  RAF and marks the message stopped.
- **Graph drag**: pointer/touch; a real *drag* must not trigger note-open (only a click without
  movement opens). Sim stays warm while dragging, cools after release.
- **Tab DnD**: each tab carries `{id, pane}`; dropping on the other pane moves it (no duplication;
  if already present in target, just removes from source). New notes open only in the **active** pane.
- **Edit/Preview** per-pane; `⌘E` toggles the active pane. Editing raw marks the tab dirty (•).
- **Keyboard**: `⌘K` palette, `⌘⇧G` graph, `⌘\` split, `⌘R` reading, `⌘E` edit/preview, `⌘S` save,
  `Esc` exits reading/closes overlays.
- All hover/active states and entrance animations are defined in the CSS; tactile press = `scale(0.94)`
  on buttons, cards lift on hover, lists stagger in. Respect `prefers-reduced-motion`.

## State Management
Prototype keeps everything in React state in `app.jsx`. Real app needs:
- theme, lang, accent, density, chrome, tweak prefs (persisted).
- Per-pane: `tabs[]`, `activeTab`, `mode(edit|preview)`. Split state: `secondPane(null|graph|editor)`,
  `splitTab`, `activePane`, widths.
- Editor: note bodies + dirty map + edited-body overrides; backlinks computed from `[[links]]`.
- AI: messages, streaming flag, provider (local/cloud), offline flag, RAG source style.
- Indexing progress, sync/conflict status. Home widgets are data-driven (brief/questions/drift are
  LLM-generated; recent/goals/stats/stale are derived from the vault).

## Assets
- **Icons**: inline monoline SVG, Lucide geometry, `currentColor`, stroke 1.75 (`icons.jsx`). No raster.
- **Brand mark**: the constellation logo (4 linked nodes) — inline SVG, terracotta squircle; also the
  favicon (data-URI in the HTML heads). No external file.
- **Fonts**: Onest + JetBrains Mono + Source Serif 4 via Google Fonts (self-host for production/offline).

## Files (in this bundle)
- `tokens.css` — **port first**: the whole palette/type/space/motion token layer (light+dark, accents).
- `app.css`, `ai.css`, `graph.css`, `conflict.css`, `plugins.css`, `onboarding.css`, `motion.css`,
  `home.css` — component/screen styles consuming the tokens.
- `app.jsx` — composition, titlebar, status bar, all state, tweaks wiring.
- `sidebar.jsx`, `editor.jsx`, `ai-panel.jsx`, `palette.jsx`, `graph.jsx`, `conflict.jsx`,
  `plugins.jsx`, `onboarding.jsx` — feature components.
- `icons.jsx` — icon set + brand mark. `data.jsx` — i18n strings + mock vault/notes. `logic.jsx` —
  backlinks, markdown render, mock AI. `tweaks-panel.jsx` — in-app tweak controls.
- `Nexus.html` — main workspace entry. `Nexus Home.html` — Home dashboard. `Onboarding.html` — onboarding.

Open the `.html` files in a browser to see the live reference. Start any port from `tokens.css`.
