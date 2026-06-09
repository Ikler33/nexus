# HOME-бэкенд + замороженный инженерный бэклог (2026-06-09)

> Разделение работ: **бэкенд HOME — здесь** (этот трек), **визуал HOME — в дизайн-чате** (поверх этого
> API). Этот документ — (1) фиксация инженерного бэклога, (2) sliced-план HOME-бэкенда, (3) **контракт
> данных** для дизайн-чата (что вызывать из фронта).

## 1. Замороженный инженерный бэклог

**✅ Закрыто (кросс-план Wave A/B + LLM, сессия 2026-06):** все Wave A; Wave B — `#9` `AppError`, `#12`
git-integration, `#13` rebuild-FTS, `#17` chat-persist, `#27` DNS-гард, `#28` декомпозиция indexer,
`#11` настройки-UI; eval-фикстура (реальное качество в CI); LLM — R2 (без reasoning для примитивов),
`ai.fast` (Qwen-утилитарка), R1-backend (живая 💭-сводка). 10 PR в main.

**🟡 Осталось автономного, НИЗКОЙ ценности (брать по желанию):** `#22` пагинация `list_notes` (спорно),
`#25` discriminated Buffer (под граф-во-вкладку), `#3` de-risk `tauri build`; perf-эпик `#14` токенайзер
→ `#15` батчинг → `#6` квантизация (`#14` полу-owner: нужен `/tokenize` или выбор либы).
**✅ Закрыто (2026-06-09):** `#18` per-path coverage-ратчет, `#10` выборочный git-стейдж (`commit_paths`).

**🟣 В работе (другой чат):** перенос дизайна + R1b (фронт 💭).

**⏸️ Owner-gated (нельзя автономно):** `#29` подпись (отложено) → `#30` updater → `#26` release → `#31`
E2E; `#16` egress-ADR/web-агент; vision→AC (умные шаблоны / News Feed / карта компетенций); `#24`
граф live-drag (sign-off); `#19` cold-bench.

**Вывод:** чисто-автономный код почти исчерпан. **Активный трек — HOME-бэкенд** (ниже). Дальше — vision
(после сессии vision→AC) и релизная инфра (после подписи).

## 2. HOME — что уже есть в бэкенде (переиспользуем)

- **Цели** (`#goal`): `commands::goals::list_goals` → `Vec<Goal{path,title,progress}>`. ✅
- **Недавние заметки:** паттерн `digest::recent_notes` (по `updated_at`). ✅ (вынесем переиспользуемо)
- **Дайджест** (= «Daily brief» зоны 2): `digest` kind планировщика + `get_latest_digest`. ✅
- **Противоречия, связанные/беклинки, поиск** — команды есть. ✅
- **Планировщик** (ADR-007): kinds + on-open(run-if-overdue)/scheduled(recurring)/on-change/manual — основа
  refresh-режимов LLM-виджетов. ✅

## 3. Sliced-план HOME-бэкенда (design-independent)

- **H1 — статические/динамические виджеты (без LLM, без кэша).** Команда `get_home_data` →
  `{ stats, recent, goals }`. `stats` = счётчики базы (заметки/теги/связи/слова); `recent` = топ-N по
  `updated_at`; `goals` = `list_goals`. Чистый SQL, мгновенно. **← первый срез.**
- **H2 — кэш LLM-виджетов + refresh-режимы (Фундамент).** Таблица `home_widgets` (key → content,
  generated_at, source_hash, status) + инвалидация по `max_file_mtime`; режимы on-open (run-if-overdue),
  scheduled (recurring раз/сутки), manual (команда) — поверх планировщика ADR-007. Команда
  `get_widget(key)` (кэш) + `refresh_widget(key)` (manual). **✅ реализовано** (`home::widgets`: трейты
  `WidgetGenerator`/`WidgetSink` + обобщённый kind `WidgetHandler` «генерация→кэш→событие»; событие
  `home:widget-updated`; `WidgetRegistry` известных ключей в `VaultContext`). Конкретные виджеты
  регистрируются в `open_vault` поверх этого слоя — H3+.
- **H3 — Daily brief** (LLM, on-open) — экспонировать существующий `digest` как home-виджет (или новый
  kind на `chat_fast`/gemma — большой контекст). **✅ реализовано**: без дублирования генерации —
  `DigestHandler` зеркалит результат в кэш `home_widgets[daily_brief]` + событие `home:widget-updated`;
  `WidgetRegistry` хранит `key → kind`, `refresh_widget("daily_brief")` бэкается kind `digest` (refresh =
  регенерация, общий дедуп с панелью); бутстрап `mirror_latest_to_widget` на открытии. on-open/recurring/
  on-change наследуются от планировщика дайджеста.
- **H4 — Stale radar** (dynamic скоринг + опц. LLM-слой top-10, кэш 24ч, инвалидация по mtime).
  **✅ реализовано** (`home::stale`): слой 1 — скоринг устаревания из метаданных (возраст/`draft`/`wip`/
  просроченный `due`/нет беклинков; `evergreen` режет; `Templates`/`Archives` исключены), команда
  `get_stale_radar`, мгновенно on-open; слой 2 — kind `stale_radar` (manual) LLM-обогащение топ-10
  (причина/действие/подсказка), кэш `stale_cache` (миграция 009) 24ч + инвалидация по `source_mtime`,
  команда `refresh_stale_radar`, событие `home:widget-updated`.
- **H5 — Open questions** (LLM, manual) + **Context drift** (LLM, scheduled). На `chat_util`/`chat_fast`.
  **✅ реализовано** (`home::insights`, первые генераторы на фреймворке H2): Open questions — последние 20
  заметок → JSON `[{question,path}]` (путь валидируется), kind `home_widget:open_questions`, manual;
  Context drift — фокус vs цели (`#goal`/`#priority`) → абзац, kind `home_widget:context_drift`, recurring
  раз/сутки + on-open (`is_overdue`), НЕ on-change. Хелперы `tauriApi.home.openQuestions()`/`contextDrift()`.
  **HOME-бэкенд H1–H5 закрыт.**

Каждый срез — отдельный линейный PR со своим тестом + CHANGELOG. Мерж только на зелёном CI вручную.

## 4. Контракт данных для дизайн-чата (фронт вызывает это)

> По мере реализации срезов появляются в `apps/desktop/src/lib/tauri-api.ts` под `tauriApi.home.*`.

- **H1:** `tauriApi.home.data(): Promise<HomeData>` где
  `HomeData = { stats: { notes, tags, links, words }, recent: NoteRef[], goals: Goal[] }`.
- **H2+:** `tauriApi.home.widget(key): Promise<Widget|null>` (кэш), `tauriApi.home.refresh(key)` (manual),
  событие `home:widget-updated` для живого обновления (как `vault:changed`).
- Виджеты по зонам — см. `docs/design/PKM_Home_Concepts.md` (зоны 1–5, классы, триггеры).

**Принцип:** статика/динамика — мгновенно; LLM-виджеты — асинхронно из кэша, никогда не блокируют
загрузку HOME (концепт §«Принципы»).
