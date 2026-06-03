# Дизайн-система «Hermes» (Фаза 4, ADR-006)

> Источник истины визуала. Хендофф (референс-прототипы + спека) — `docs/design/handoff/`
> (`README.md` + `tokens.css` + per-screen `.css/.jsx` + `.html`). Прод-реализация — на наших
> настоящих компонентах (CodeMirror 6, sigma.js, i18n, broker), а не копия `.jsx`.

## Токены (`src/styles.css`)
Единственный источник цвета/типографики/отступов/моушна. Семантические имена **стабильны**,
per-theme меняются только значения. Компоненты ходят **только через токены** (`var(--…)`), без
хардкода цветов.

- **Палитра — OKLCH**, тонирована в единый тёплый hue (`--ui-hue`). Нейтрали, акцент
  (`--color-accent`, терракота), info-слой (`--color-ai`, teal), `--color-link`/`--color-tag`,
  статусы (`success/warning/danger`), `--color-selected` (единый цвет выделения — строки дерева,
  активные кнопки рейла), elevation, focus-ring.
- **Темы** через `data-theme` на `<html>`: `light` («old paper», тёплый крем) и `dark`
  («warm clay», тёплый уголь). Дефолт без атрибута = light.
- **Акцент** через `data-accent` ∈ `amber`(деф.)/`teal`/`sage`/`clay` — задаёт `--acc-l/c/h`.
- Типографика: `--text-xs…2xl` (11–26px, база 13–14), `--font-ui` (Onest), `--font-mono`
  (JetBrains Mono), `--font-serif` (Source Serif 4 — проза/акценты). Редактор: `--editor-*`.
- Сетка `--space-1…8` (4–48px), `--radius-sm/md/lg`, `--row-h` + `--density`, `--motion-*` + ease.

## Темы (`src/stores/theme.ts`)
Zustand-стор: `theme` + `toggle()`/`setTheme()`. Старт — из `localStorage('nexus-theme')`, иначе
системная (`prefers-color-scheme`). Применяется до первого рендера (side-effect на импорте —
`main.tsx` импортит стор до `App`), без вспышки. Смена — с 320ms кросс-фейдом (класс `.theme-anim`
на `<html>`, gated `prefers-reduced-motion`), персист в localStorage. Тоггл — кнопка в шапке
(sun/moon) + команда `theme.toggle`.

## Шрифты (`src/fonts.ts`)
**Self-hosted** через `@fontsource` (бандл, offline/local-first — НЕ Google Fonts в рантайме):
`@fontsource-variable/onest`, `@fontsource-variable/source-serif-4` (+ italic),
`@fontsource/jetbrains-mono` (400/500/600). CSP `font-src 'self' data:` (Ф0-12) их разрешает —
Vite эмитит woff2 как ассеты 'self' / маленькие как data:. Фоллбеки в `--font-*` — системные.

## Порядок внедрения (Фаза 4)
1. **Ф4-0 (этот срез) — фундамент:** порт токенов + self-host шрифтов + тема свет/тёмная (тоггл,
   persist, кросс-фейд) + акцент-пресеты. Существующий апп перекрашивается «бесплатно» (все
   компоненты на токенах).
2. **Порескринный рестайл** существующих экранов под `docs/design/handoff/*.css` (titlebar,
   sidebar/rail, editor/tabs, graph, ai-panel, palette, plugins) — пиксель-фейтфул, на наших
   компонентах.
3. **Новые экраны:** reading mode, вложения (картинки/`![[embeds]]`/Mermaid/LaTeX), **conflict
   resolver** (3-way merge — закрывает отложенный git-merge), onboarding, tweaks-панель,
   Home-дашборд (новый, в конце), экспорт PDF/Print.

## Прогресс
- [x] Ф4-0 — токены/шрифты/тема (фундамент). Существующий апп в новом облике, тоггл свет/тёмная.
- [ ] рестайл экранов · новые экраны (см. выше).
