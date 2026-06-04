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
- [x] Ф4-1 — chrome shell: titlebar (бренд + поисковая пилюля→палитра + группа инструментов) +
  status bar + сетка `38/1fr/26` (вариант A — бар в обычном OS-окне; frameless/traffic-lights позже).
- [x] Ф4-2 — рестайл сайдбара: tree-rows (фирменное выделение: selected-фон + 3px акцент-полоса +
  акцентная иконка) + поле поиска (radius-md + accent-soft focus). Rail (files/search/tags/starred) —
  отдельно (нужны панели tags/starred = новые фичи).
- [x] Ф4-3 — рестайл вкладок редактора: floating tabs (активная приподнята до холста + `--tab-shadow`
  + 2px акцент-полоса сверху; фокус-группа = акцент, иначе приглушённая). Edit/Preview-pill +
  центр-measure редактора — отдельно (нужен preview-режим = фича).
- [x] Ф4-4 — рестайл графа: цвета узлов/рёбер из токенов (центр = accent, соседи = text-muted, рёбра =
  border-strong) через 1×1-canvas readback (sigma WebGL не парсит oklch) + радиальный фон холста.
- [x] **Граф: интерактив (по дизайну `graph.jsx`)** — sigma.js заменён на кастомный SVG force-directed:
  drag (соседи подтягиваются), hover-подсветка, **пульс/halo/ripple/кольцо активной ноты** (отложенное в
  Ф4-4 — сделано), kin-кольца, «поток» по рёбрам, local/full + глубина. Логика — `graph-sim.ts` (тесты),
  визуал — human-verify. Теги-цвета/фильтр — отдельным срезом (нужны теги на узлах).
- [x] Ф4-5 — рестайл Command Palette: glass-модал (blur-скрим + стеклянная палитра, accent-soft
  активная, kbd-хинты) + staggered-раскрытие строк (--cmd-i).
- [x] Ф4-6 — рестайл AI-панели: провайдер-пилюля + прозрачные кнопки, пузырь юзера accent-soft,
  источники-карточки с AI-бейджем, композер — плавающий бокс с accent-soft focus-кольцом.
- [x] Ф4-7 — рестайл панели плагинов (демо+аудит): glass-бэкдроп, elevated-диалог, чип-пилюля,
  прозрачный close, аудит на chrome (allowed→success/denied→danger). Менеджер+consent-sheet — post-v1.
- [x] **Рестайл существующих экранов завершён.** Дальше — новые экраны:
- [x] Ф4-8a — conflict resolver **бэкенд** (libgit2 in-memory 3-way merge: preview base/ours/theirs +
  apply→merge-коммит; команды + api + мок + Rust-тест). Закрывает git-хвост Ф3 (бэк).
- [x] Ф4-8b — conflict resolver **UI** (3-way панель: НАШЕ/ИХ + редактируемый результат → merge-коммит
  + push). **git-хвост Ф3 закрыт.** Проверено в превью (полный поток).
- [x] Ф4-9 — режим чтения (⌘R): прячет сайдбар/AI, редактор full-width; команда + Esc-выход. Центр-measure — рефайнмент.
- [x] Ф4-10 — просмотр не-md вложений (картинки/PDF во вкладке через asset-URL; бинарь не читается как
  текст). **Inline-рендер/Mermaid/LaTeX → эпик Live Preview (BACKLOG).**
- [x] Ф4-11 — онбординг (первый запуск): приветственный экран при отсутствии vault (бренд + CTA «Открыть
  vault» + язык/тема); авто-открытие убрано. Многошаговый flow — рефайнмент.
- [x] Ф4-12 — панель оформления (tweaks): тема / акцент (amber/teal/sage/clay → data-accent) / плотность
  (--row-h). Стор оформления + titlebar-кнопка + команда. Проверено в превью (teal перетинтовал апп).
- [x] **Рестайл + новые экраны Ф4 завершены** (Home — в BACKLOG).
- [x] Ф4-13 — печать / экспорт PDF активной заметки (`file.print` + print-CSS; печатает исходник, рендер → Live Preview эпик).
- [x] Ф4-14 — локальный crash-reporter (panic-hook → scrubbed-лог `~/.nexus/crashes/`, без сети). Помогает на тестировании.
- [x] **Инфра C — выполнимое сделано** (print-экспорт + локальный crash-лог). Остальное в BACKLOG:
  auto-updater (подпись) · crash-backend (opt-in) · `nexus-md-parser` пакет · скелет `apps/mobile/`
  (релиз-время / старт мобильного трека). **Разработка по A+B+C завершена → ручное тестирование.**
- [ ] новые экраны: reading mode · вложения/Mermaid/LaTeX · conflict resolver · onboarding · tweaks · Home.
