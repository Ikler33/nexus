# Планировщик фоновых задач (ADR-007)

> Очередь `jobs` для фоновых LLM/индексных задач (News Feed, Карта компетенций, Поиск противоречий,
> память агента…). Решения зафиксированы owner-codesign (`docs/reviews/ADR_CODESIGN.md`, секция S1–S10).
> Строится срезами; **сейчас готов slice 1 — слой данных очереди**.

## Статус по срезам

| Срез | Что | Статус |
|---|---|---|
| **1. Слой данных** | таблица `jobs` (миграция 004) + `scheduler::{enqueue, claim_next, complete, fail, requeue_running, gc_done}` | ✅ |
| **2. Движок диспатча** | `JobHandler`-трейт + `Registry` (kind→handler), `run_due`, воркер-луп `spawn_worker` (tokio-interval S1, `jobs:changed` event) | ✅ (не спавнится live до среза 3) |
| 3. Триггеры + live-спавн | on-open / on-change (от завершения реиндекса, S4) / scheduled (run-if-overdue, S2); `spawn_worker` в `open_vault` | ⏳ |
| 4. Первые kind + backpressure | Карта компетенций, Поиск противоречий (локальные, несетевые — S3); приоритет чата над LLM-джобами (S5) | ⏳ |
| 5. UI | StatusBar N/M поверх `jobs:changed`, видимый `dead`/pending (S7/S8) | ⏳ |

Сетевые kind (News Feed и весь web/cloud-класс) заблокированы на egress (#16) — отдельная волна.

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

## Зависимости (закрыты)
- **#13 rebuild-примитив миграций** — ✅ (`jobs`-миграция идёт через раннер с `rebuild_fts`-хуком).
- **Event-канал backend→фронт** — ✅ (`vault:changed`; срез 2/5 будет слать прогресс джоб поверх него).
