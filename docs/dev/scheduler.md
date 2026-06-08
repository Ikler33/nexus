# Планировщик фоновых задач (ADR-007)

> Очередь `jobs` для фоновых LLM/индексных задач (News Feed, Карта компетенций, Поиск противоречий,
> память агента…). Решения зафиксированы owner-codesign (`docs/reviews/ADR_CODESIGN.md`, секция S1–S10).
> Строится срезами; **сейчас готов slice 1 — слой данных очереди**.

## Статус по срезам

| Срез | Что | Статус |
|---|---|---|
| **1. Слой данных** | таблица `jobs` (миграция 004) + `scheduler::{enqueue, claim_next, complete, fail, requeue_running, gc_done}` | ✅ |
| **2. Движок диспатча** | `JobHandler`-трейт + `Registry` (kind→handler), `run_due`, воркер-луп `spawn_worker` (tokio-interval S1, `jobs:changed` event) | ✅ |
| **3. Live-спавн + первый kind** | `spawn_worker` в `open_vault` (crash-recovery на старте); встроенный kind **`gc`** (`default_registry`) + seed на открытии — конвейер живой end-to-end | ✅ |
| **4a. Первый LLM-kind** | kind **`digest`** «Дайджест изменений» (#35): недавние заметки → chat → таблица `digests` (миграция 005); seed run-if-overdue (S2) на открытии; команды `get_latest_digest`/`generate_digest` | ✅ |
| **4b. UI дайджеста** | панель дайджеста из титлбара + команда `view.digest` (refetch по `jobs:changed`, без поллинга); `tauriApi.digest.*` + `events.onJobsChanged`; стор `digest` | ✅ |
| **5. backpressure + StatusBar** | приоритет чата/inline над LLM-джобами (S5: `defer_under_interactive` + `is_interactive_busy`); индикатор задач в StatusBar (running/pending/dead) поверх `jobs:changed` | ✅ |
| 6. расписание + on-change | дедуп одинаковых kind в очереди; on-change-триггер (S4); периодическое расписание (S2 демон) | ⏳ |

Сетевые kind (News Feed и весь web/cloud-класс) заблокированы на egress (#16) — отдельная волна.

**Live (slice 3):** `open_vault` строит `default_registry` (kind `gc`), спавнит воркер (как индексатор) и
сидит gc-джобу на ближайший тик → доказывает живой конвейер (spawn → enqueue → claim → выполнение →
`done` → `jobs:changed`). Сейчас seed-на-открытие + 5с-тик; **дедуп/run-if-overdue-расписание + on-change**
и **первый LLM-kind** (Карта/Противоречия, на живых моделях) — срез 4. **Грабли (как у индексатора):**
воркер спавнится на каждый `open_vault` → при переоткрытии vault возможны дубли-воркеры на старый
write-actor (лог-шум, не корраптит) — нужен shutdown-сигнал (BACKLOG, общий с индексатором).

## Схема (`migrations/004_jobs.sql`)
`jobs(id, kind, payload, state, run_at, attempts, max_attempts, last_error, created_at, updated_at)` +
`idx_jobs_claim(state, run_at)`. Состояния: **`pending → running → done | dead`**. `payload` — JSON-параметры
(агностично к kind). Новая таблица производных не инвалидирует → `rebuild_fts: false`.

## Слой данных (`scheduler/mod.rs`)
- **`enqueue(kind, payload, run_at, max_attempts)`** — ставит `pending` (не раньше `run_at`).
- **`claim_next(now)`** — берёт первую готовую (`pending` и `run_at<=now`), помечает `running`, отдаёт `Job`.
  **Без гонок**: единственный write-actor (ADR-003) сериализует claim — SELECT+UPDATE в одной транзакции.
- **`complete(id)`** → `done`.
- **`fail(id, error, now)`** — `attempts++`; по исчерпании `max_attempts` → **`dead`** (видимый, S7 — НЕ тихий
  дроп); иначе → `pending` с экспоненциальным backoff (`run_at = now + 30·2^attempts`, cap 3600с).
- **`requeue_running()`** — crash-recovery: «зависшие» `running` → `pending` (на старте, S8).
- **`gc_done(before)`** — чистит старые `done` (S7 — `idx_jobs_claim` не деградирует).

Логически значимое время (`run_at`/backoff) — **явные параметры** → детерминированные тесты;
`created_at/updated_at` — внутренним `now_secs()`.

## Движок диспатча (slice 2)
- **`JobHandler`** (`#[async_trait]`): `async fn handle(&self, job) -> Result<(), String>`. Реализация
  держит свои зависимости (db/embedder/chat). **`Registry = HashMap<kind, Arc<dyn JobHandler>>`**.
- **`run_due(writer, registry, now)`** — детерминированное ядро тика: `claim_next` → диспатч по `kind` →
  `complete` (Ok) / `fail` (Err); неизвестный `kind` → `fail` (после ретраев — `dead`). Не более
  `MAX_PER_TICK=64` за вызов (анти-голодание; излишек — на следующие тики).
- **`spawn_worker(writer, app, registry)`** — воркер-луп: crash-recovery на старте → `tokio::interval`
  (TICK=5с) → `run_due`; после продуктивного тика шлёт `jobs:changed`. Пока **не спавнится из
  `open_vault`** (нет kind/энкьюеров) — live-спавн в срезе 3, чтобы пустой воркер не dead-летил джобы.
  **Backpressure чата (S5)** — со срезом LLM-kind (сейчас конкурентов за LLM нет).

## Тесты (`scheduler::tests`)
`claim_respects_run_at_and_completes`; `fail_retries_with_backoff_then_dead` (backoff→готова→dead);
`requeue_and_gc` (running→pending; GC); **`run_due_dispatches_by_kind`** (ok→done, падающий→backoff,
неизвестный kind→dead). Все офлайн на temp-БД (без сети/LLM).

## Первый LLM-kind: «Дайджест изменений» (slice 4a, модуль `digest`)
- **`KIND_DIGEST="digest"`**, окно `WINDOW_SECS=24ч`, `MAX_NOTES=40`, `SNIPPET_CHARS=200`.
- **`DigestHandler{reader, chat, writer}`** (impl `JobHandler`): `recent_notes` (files.updated_at ≥ since +
  сниппет первого чанка) → если пусто, `Ok(())` без записи → иначе `build_prompt` → `chat.stream_chat`
  (не-стрим: no-op sink, берём полный текст) → `store` в `digests`. Регистрируется в `open_vault`
  **только если chat сконфигурирован** (иначе kind отсутствует — нет тихих dead-джоб).
- **`should_generate(reader)`** — нет ли дайджеста за последнее окно; на `open_vault` сидим джобу только
  при `true` (**run-if-overdue**, S2 — без расписания-демона пока).
- **Команды** (`commands/digest.rs`): `get_latest_digest` → `Option<Digest>`; `generate_digest` —
  enqueue (требует chat, иначе понятная ошибка).
- **Таблица `digests`** (миграция 005): `id, created_at, since, content, note_count` + `idx_digests_created`.
  Не входит в allowlist `count_tables`-теста (как и `jobs`).
- **Тесты** (`digest::tests`, офлайн, `FakeChat`): `summarizes_recent_notes_and_stores` (генерит + снимает
  overdue), `no_recent_notes_no_digest` (пустой vault → без записи).
- **UI (slice 4b):** модалка `DigestPanel` из титлбара + команда палитры `view.digest`; показывает
  последний дайджест (контент + мета) и кнопку «Сгенерировать» (enqueue). Готовый результат прилетает
  по `events.onJobsChanged` → refetch (только когда панель открыта, без поллинга); кнопка в состоянии
  «Генерирую…» до прихода дайджеста свежее baseline. Контракт: `tauriApi.digest.latest()/generate()`.
- **Дальше:** backpressure чата (S5); дедуп одинаковых `kind` в очереди; on-change-триггер (S4);
  StatusBar N/M (slice 5).

## Backpressure + StatusBar (slice 5)
- **S5 backpressure.** `JobHandler::defer_under_interactive()` (по умолчанию `false`; `DigestHandler` →
  `true`). `AppState::interactive_llm` — счётчик активных интерактивных LLM-операций; `chat_rag` и
  `inline_complete` берут RAII-гард `enter_interactive_llm()` вокруг стрима. Воркер каждый тик читает
  `is_interactive_busy()` и передаёт `busy` в `run_due`; при `busy` тяжёлые LLM-джобы **откладываются**
  (`defer`: `running → pending`, `run_at = now + TICK`, **без** штрафа `attempts`) — дайджест уступает
  чату/inline за локальную модель. Лёгкие (gc) идут всегда.
- **StatusBar.** `scheduler::counts(reader) → JobCounts{pending,running,dead}` (один GROUP BY; `done` не
  считаем — их чистит gc). Команда `get_job_counts` (нет vault → нули, не ошибка). Фронт: стор `jobs` +
  индикатор в StatusBar (⚙ running · ⏳ pending · ⚠ dead), refetch по `events.onJobsChanged` (без поллинга).
- **Тесты:** `run_due_defers_llm_job_under_interactive` (busy → отложена/не выполнена; !busy → выполнена);
  `counts_reports_states`; фронт `StatusBar.test.tsx` (индикатор по данным / пусто → скрыт).
- **Дальше (slice 6):** дедуп одинаковых `kind` в очереди, on-change-триггер (S4), периодическое
  расписание (S2-демон вместо только run-if-overdue на открытии).

## Зависимости (закрыты)
- **#13 rebuild-примитив миграций** — ✅ (`jobs`-миграция идёт через раннер с `rebuild_fts`-хуком).
- **Event-канал backend→фронт** — ✅ (`vault:changed`; срез 2/5 будет слать прогресс джоб поверх него).
