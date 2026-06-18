# Эпизодическая память агента Nexus

Спецификация. Статус: decision-complete. Версия 1.0 (2026-06-18). Синтез мультиагентного дизайн-воркфлоу (4 архитектуры → adversarial-критика → судья). Победившая философия «**Сессия = эпизод**» как костяк + устранение трёх фатальных изъянов lifecycle-слоя, подтверждённых по живому коду.

---

## 0. Резюме (TL;DR)

Эпизод — связное нарративное саммари **одной завершённой чат-сессии** («о чём был разговор и к чему пришли»), хранимое в `chat_episodes` (1:1 с `chat_sessions`) + параллельный usearch-индекс `episode_vectors`. Генерируется фоном **через scheduler-джобу `episode_rollup`** (recurring scheduled-only, run-if-overdue на открытии vault) — НЕ через in-memory debounce. Инжектируется в чат отдельным каналом `EpisodeSources` (зеркало `MemorySources`), под тогглом `aiEpisodicMemory` (**OFF по умолчанию**). Вся запись аддитивна, обратима (soft `dismissed` + жёсткое удаление командой `episode_purge`, не CASCADE), под eval-гейтом faithfulness. Ноль нового egress.

---

## 1. Vision и место в архитектуре памяти

Nexus имеет ТРИ независимых слоя памяти:

| Слой | Что | Единица | Ключ/таблица | Вопрос | Курация |
|---|---|---|---|---|---|
| **Факты** (MEM) | атомарные курируемые утверждения о пользователе | факт | `memory_facts` / `memory_vectors` | «кто пользователь» (вечно) | пользователь (explicit/auto + consolidate-гейт) |
| **N4b** (память переписки) | сырые отдельные сообщения | реплика | `chat_messages.id` / `chat_vectors` | «что именно было сказано» (дословно) | нет (RAG) |
| **Эпизод** (EP) | нарратив одной сессии | сессия | `chat_episodes` / `episode_vectors` | «что это была за встреча и чем кончилась» | soft-dismiss + purge |

Эпизод **не нормализуется и не дедуплицируется** между собой (привязка ко времени — его ценность; дедуп нарративов разных дней = потеря датировки). Поэтому `consolidate.rs`/MEM-8 для эпизодов **НЕ переиспользуется** (он про атомарные факты). Эпизод **не заменяет** `propose_facts` — дополняет.

**Потолок философии (осознанная плата за скорость/безопасность):** эпизод покрывает только чат-сессии. File-активность (`edit_events`), сшивание межсессионных сюжетов и rollup-рефлексия (эпизоды-из-эпизодов) — owner-gated расширения (§11), не часть EP-1..4.

---

## 2. Что такое эпизод

Эпизод = строка `chat_episodes`, 1:1 с `chat_sessions.id`:
- `summary` — связное саммари 3–6 предложений (RU), производное от транскрипта сессии;
- `topics` — JSON-массив тем (чипы UI + keyword-fallback);
- метаданные свежести/идемпотентности (`last_msg_id`, `msg_count`), времени (`started_at`/`ended_at`), аудита (`model`/`embed_model`/`generated_at`), обратимости (`dismissed`).

Эпизод **производный и восстановимый**: первоисточник (`chat_messages`) остаётся всегда; таблицу и индекс можно дропнуть — эпизоды пере-сгенерируются rollup-джобой, первоисточник не теряется.

---

## 3. Схема БД — миграция 019

`apps/desktop/src-tauri/src/db/migrations/019_chat_episodes.sql` (version=19, head=18, rebuild_fts=false). Регистрация в `db/migrations.rs`: `Migration { version: 19, name: "chat_episodes", sql: include_str!("migrations/019_chat_episodes.sql"), rebuild_fts: false }`.

```sql
-- Эпизодическая память (EP): эпизод = саммари ОДНОЙ чат-сессии. 1:1 с chat_sessions (session_id UNIQUE).
-- Все поля ПРОИЗВОДНЫ от chat_messages → таблицу можно дропнуть/пересобрать без потери первоисточника.
-- ON DELETE CASCADE — лишь корректность на случай будущего удаления сессии; ОСНОВНОЙ путь полного
-- удаления эпизода — ЯВНАЯ команда episode_purge (в коде нет delete-session; «храним всё»).
CREATE TABLE IF NOT EXISTS chat_episodes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    INTEGER NOT NULL UNIQUE REFERENCES chat_sessions(id) ON DELETE CASCADE,
    summary       TEXT    NOT NULL,            -- связное саммари (RU): вход эмбеддинга + UI/инъекция
    topics        TEXT,                        -- JSON-массив строк-тем; NULL до заполнения
    msg_count     INTEGER NOT NULL,            -- покрытых сообщений (idempotency: пересжимаем при росте)
    last_msg_id   INTEGER NOT NULL,            -- max(chat_messages.id) на момент генерации — водяной знак
    started_at    INTEGER NOT NULL,            -- min(created_at) сессии — time-range ретривал
    ended_at      INTEGER NOT NULL,            -- max(created_at) сессии — time-range ретривал
    model         TEXT,                        -- chat_util|chat_fast — аудит/рекалибровка
    embed_model   TEXT,                        -- модель эмбеддинга summary (реконсиляция при смене, §6)
    generated_at  INTEGER NOT NULL,
    dismissed     INTEGER NOT NULL DEFAULT 0   -- мягкое скрытие (обратимо); НЕ сбрасывается пересжатием
);
CREATE INDEX IF NOT EXISTS idx_chat_episodes_ended   ON chat_episodes(ended_at DESC);
CREATE INDEX IF NOT EXISTS idx_chat_episodes_session ON chat_episodes(session_id);
CREATE INDEX IF NOT EXISTS idx_chat_episodes_live    ON chat_episodes(dismissed, ended_at DESC);
-- Семантический индекс — НЕ в SQLite: episode_vectors.usearch (ключ = chat_episodes.id),
-- открывается рядом с chat_vectors/memory_vectors в commands/vault.rs::build_rag.
```

---

## 4. Генерация эпизода (фикс flaw #3 — scheduler, не debounce)

### 4.1. Механизм
In-memory debounce-map отвергнут: (а) новый конкуррентный примитив; (б) НЕ переживает рестарт → короткие сессии, закрытые до idle, **никогда** не суммируются (тихая дыра покрытия). Используем проверенные паттерны кода: `recurring` map + `run-if-overdue` seed + backfill-via-`contains()`.

- Новый scheduler `kind = "episode_rollup"` + `EpisodeRollupHandler`.
- **`defer_under_interactive() = true`** (суммаризация уступает интерактивному чату — backpressure, как digest).
- **recurring scheduled-only** (НЕ on-change): `recurring.insert("episode_rollup", EPISODE_ROLLUP_INTERVAL)`, `EPISODE_ROLLUP_INTERVAL = DAY_SECS/4` (≈6 ч, калибруемо). Эпизод — «успокаивающийся» сигнал.
- **seed run-if-overdue на открытии vault**: джоба ставится только если `has_stale_episodes` И не запланирована немедленная (`!has_ready_job`) И тоггл ON.
- Тоггл `aiEpisodicMemory` гейтит seed И handler (ранний NOOP). OFF = ноль LLM-вызовов и записи.

### 4.2. Гейт «созревшей» сессии
`handle` обрабатывает до `EPISODE_BATCH = 5` кандидатов за прогон. Кандидат-сессия:
- `(а)` `msg_count_now ≥ EPISODE_MIN_MSGS` (=4: не суммируем пинги);
- `(б)` «успокоилась»: `now − max(created_at) ≥ EPISODE_QUIET_SECS` (=2 ч);
- `(в)` НЕ актуальна: нет строки `chat_episodes` с `last_msg_id == max(chat_messages.id)` сессии (idempotency).

Детерминированный SQL, юнит-тестируем. `has_stale_episodes` = «есть ≥1 такой кандидат».

### 4.3. Идемпотентность и анти-гонка
- `INSERT ... ON CONFLICT(session_id) DO UPDATE SET ...` (атомарно, last-write-wins по `last_msg_id`).
- Гонка двойного спавна исключена **архитектурно**: единственный писатель — scheduler-воркер (один на vault, `claim_next` сериализует).
- При UPDATE-пересжатии `dismissed` **НЕ в SET-списке** (фон не отменяет намерение юзера скрыть).

### 4.4. Модель и промпт
- `chat_util` (Qwen3-4B) с фолбэком `chat_fast` (паттерн `set_title`), без reasoning, t≈0.2. Не-стрим через `stream_chat` с no-op token sink (образец `DigestHandler`).
- System-промпт: «Связное саммари 3–6 предложений по-русски: о чём спрашивали, к чему пришли. Опирайся ТОЛЬКО на диалог между маркерами — не выдумывай. Затем строкой `Темы: a, b, c`.» Транскрипт **обёрнут `injection_marker()`** (анти-инъекция на входе).

### 4.5. Обработка ошибок LLM
Best-effort: ошибка/таймаут/пустой ответ → эпизод **не пишется**, джоба `Ok(())` (не ошибка пайплайна, рекуррентность доберёт). Спиннеру негде залипнуть (фоновая джоба, не виджет). Если позже добавится UI-индикатор — гасить в ЛЮБОМ исходе (урок AIP-5), future-scheduled (run_at>now) не считать «работающей» (урок #63).

---

## 5. Ретривал и инъекция в чат

### 5.1. Поиск
`episode::search_episodes(reader, episode_vectors, embedder, &question, EPISODE_K, exclude_sessions, snippet_chars)` — зеркало `chat_log::search_memory`: `embed_query` → `episode_vectors.search(qvec, (K*4).max(8))` → отсечь ниже порога → resolve в `EpisodeHit` (JOIN за title), фильтр `dismissed=0` → исключить текущую сессию → `truncate(K)`. `EPISODE_K = 2`.

### 5.2. Калибровка порога
`EPISODE_SIM_THRESHOLD` **НЕ наследует** MEM 0.30 (длинное саммари ≠ короткий факт на bge-m3). Старт **0.45**, финал — из offline-eval (EP-4).

### 5.3. Дедуп между EpisodeHit и MemoryHit
Кейс: вариация старого вопроса в НОВОЙ сессии → эпизод старой сессии И сырые `chat_vectors`-реплики той же старой сессии всплывают вместе → один разговор и пересказан, и процитирован. **Решение:** сначала собираем `EpisodeHit`, множество `episode_session_ids`, при сборке `MemoryHit` **исключаем** сессии из него (расширить `search_memory` параметром `exclude_sessions: &HashSet<i64>`). На сессию идёт ЛИБО эпизод, ЛИБО реплики.

### 5.4. Инъекция и токен-бюджет
- `build_episode_block(hits, marker)` — сосед `build_memory_block`. Каждый эпизод: `«Прошлый разговор «{title}» ({дата}): {summary}»`, весь блок в `injection_marker()` (двойная анти-инъекция). Summary обрезается `truncate_chars(.., EPISODE_INJECT_MAX_CHARS=400)`.
- **Общий токен-бюджет** `MEMORY_PREPEND_BUDGET_CHARS` (≈2500): блоки в порядке приоритета **пины-факты → эпизоды → N4b**, сборка прекращается при исчерпании (свежий note-контекст не вытесняется).

### 5.5. Стрим-событие
`ChatStreamEvent::EpisodeSources { sources: Vec<EpisodeHit> }` (рядом с `MemorySources`), до токенов. `EpisodeHit` (camelCase): `episode_id, session_id, session_title, summary_snippet, started_at, ended_at, score`.

---

## 6. Реконсиляция эмбеддера и orphan-вектор (фикс flaw #2)

`reconcile_embedding_model` при смене модели чистит `vectors.usearch`, но НЕ `chat_vectors`/`memory_vectors` — латентная дыра, которую episode унаследовал бы 1:1 (DimMismatch-ошибка или семантический мусор). Фикс:
1. `reconcile_embedding_model` дополнительно `remove_file(episode_vectors.usearch)` + `UPDATE chat_episodes SET embed_model=NULL` (summary остаётся — переэмбеддинг дёшев).
2. **Seed-backfill эпизодов на открытии** (зеркало chat_vectors-бэкфилла): `tokio::spawn`, переэмбеддит `summary` где `!episode_vectors.contains(id)` ИЛИ `embed_model != current`. Best-effort.
3. **Vector-GC при purge**: `episode_purge` зовёт `episode_vectors.remove(id)` + `save()`.

Пред-существующая orphan-дыра `chat_vectors`/`memory_vectors` — отдельная owner-flagged задача (не scope EP).

---

## 7. Безопасность и обратимость

- **Приватность:** всё локально (chat_util на 192.168.0.31, эмбеддинг по консентнутому `EgressFeature::Embed`, хранение в per-vault `.nexus`). Ноль нового egress/хостов, ноль ослабления CSP.
- **Анти-инъекция (двойная):** транскрипт при генерации + готовое summary при ретривале — оба в `injection_marker()`.
- **Обратимость (фикс flaw #1):** `dismissed=1` — soft-hide (обратимо `episode_restore`, не сбрасывается пересжатием); **`episode_purge(id)`** — жёсткое удаление (`DELETE` строки + `episode_vectors.remove` + `save`), реальный путь стереть summary+вектор (CASCADE мёртв — delete-session нет).
- **Eval-гейт faithfulness (БЛОКИРУЮЩИЙ на EP-2):** запись аддитивна → жёсткий гейт на запись не нужен. Но ложная память — вред, Qwen3-4B слабее gemma → `episode_summary_faithfulness` БЛОКИРУЕТ включение РЕТРИВАЛА (EP-2 не мержится, пока `live_episode_summary_meets_gate` не зелёная, `MIN_EPISODE_FAITHFULNESS=0.85`, ≥20 кейсов). Рекалибровка при смене модели.
- **Fail-closed:** ошибка генерации → не пишем; ошибка ретривала → чат без эпизодов. Тоггл OFF дефолт.
- **Мок зеркалит контракт точно** (урок MEM-5): `purge` (освобождает строку+вектор), `dismiss` (только скрывает, пересжатие не сбрасывает).

---

## 8. Ретенция, консолидация, eval

- **Накопление:** 1 эпизод/сессия — на порядки медленнее chat_vectors. Ретенция = как у сессий («храним всё»). Освобождение — только `episode_purge`.
- **Консолидация между собой:** НЕ делаем (дедуп нарративов = потеря датировки).
- **Eval-харнес** `eval/episodes.rs::episode_summary_faithfulness` по образцу `eval/consolidation.rs`: фиктивный предиктор доказывает, что гейт ловит галлюцинацию БЕЗ LLM; live-точка `live_episode_summary_meets_gate`. Golden `eval/episode_eval.json`.

---

## 9. UI

- **Панель «Эпизоды»** (вкладка AI-панели) — таймлайн обратной хронологии по `ended_at`: карточка = дата-диапазон + `session_title` + summary (раскрытие) + чипы `topics`. Клик → грузит сессию (`chat_session_messages`). «Скрыть» → `episode_dismiss` (undo-тост). «Удалить навсегда» → `episode_purge` (подтверждение). focus-trap по `MemoryPanel` (`useFocusTrap`, `TRAP_OVERLAYS_CLOSED`). Сверка вида с `MemoryPanel` ДО постройки (урок confirm_ui_before_building).
- **В чате:** `EpisodeSources` как `MemorySources` — «из прошлого разговора» + дата, клик открывает сессию.
- **Настройки→AI:** тоггл «Эпизодическая память» (`aiEpisodicMemory`, OFF). i18n RU+EN (`episode.*`).

---

## 10. Реестр РЕШЕНИЙ

| # | Развилка | Выбор | Обоснование |
|---|---|---|---|
| D-EP-1 | Единица эпизода | сессия (1:1) | человек так и помнит сессии; медленный рост |
| D-EP-2 | Слить с фактами/N4b? | Нет, 3-й слой | разные вопросы; дедуп нарративов теряет датировку |
| D-EP-3 | Генерация | scheduler-джоба, НЕ debounce | debounce не переживает рестарт (дыра покрытия) |
| D-EP-4 | recurring vs on-change | scheduled-only + seed | эпизод — «успокаивающийся» сигнал |
| D-EP-5 | «сессия завершена» | QUIET(2ч)+MIN_MSGS(4)+idempotency | не пинги/активный; не жжём LLM повторно |
| D-EP-6 | Полное удаление | `episode_purge`, НЕ CASCADE | delete-session нет → CASCADE мёртв |
| D-EP-7 | dismissed при пересжатии | НЕ сбрасывать | фон не отменяет намерение скрыть |
| D-EP-8 | Реконсиляция эмбеддера | дроп `episode_vectors`+`embed_model=NULL`; backfill | иначе dim-mismatch/мусор |
| D-EP-9 | Порог сходства | отдельный 0.45, калибруется | длинное summary ≠ факт |
| D-EP-10 | Дубль с N4b | дедуп по session_id МЕЖДУ каналами | не пересказан И процитирован |
| D-EP-11 | Токен-бюджет | общий, приоритет пины→эпизоды→N4b | не вытеснять note-контекст |
| D-EP-12 | Eval-гейт | НЕ на запись; БЛОКИРУЕТ ретривал (EP-2) | запись обратима; ложная память — вред |
| D-EP-13 | Анти-инъекция | двойной marker (генератор + ретривал) | summary — производное недоверенного |
| D-EP-14 | DTO/события | `EpisodeHit`∼`MemoryHit` | единая сериализация/UI/мок |
| D-EP-15 | Тоггл | `aiEpisodicMemory` OFF | autonomy-safe мандат |
| D-EP-16 | Модель | chat_util→chat_fast, без reasoning, t≈0.2 | паттерн set_title |
| D-EP-17 | Консолидация эпизодов | НЕ делаем | дедуп теряет датировку |

---

## 11. Отложенное (owner-gated / backlog)

- **Rollup-рефлексия** (эпизоды-из-эпизодов, суточный/недельный свод, Generative-Agents) — EP-6, owner-gated.
- **Эпизоды на основе file-активности** (`edit_events`) — отдельный дизайн.
- **Межсессионные сюжеты** — A не сшивает.
- **Эпизод→кандидаты в `propose_facts`** (мост к семантической памяти) — после EP-3.
- **Time-range «on-this-day»/«на этой неделе»** как первоклассный режим — EP-5.
- **Граф/визуализация эпизодов**.
- **[owner-flagged отдельная задача]** Пред-существующая orphan-дыра `chat_vectors`/`memory_vectors` при смене эмбеддера (`reconcile_embedding_model` их не чистит) — баг N4b/MEM, не вводимый эпизодами; фиксить тем же приёмом в своём срезе.

---

## 12. Фазовый роадмап

Каждый срез: track-ветка от origin/main, полный гейт (`scripts/test-all.sh` без пайпов + `cargo fmt --check` + clippy), adversarial-ревью ПЕРЕД мержем, CHANGELOG/IMPROVEMENT_PLAN/BACKLOG/MEMORY catch-up, мерж мимо windows-флейка `0xc0000139` БЕЗ `--admin`.

- **EP-1** Фундамент: миграция 019 + генерация через scheduler-джобу (без ретривала/UI). risk: medium.
- **EP-2** Ретривал + инъекция под eval-гейтом faithfulness (БЛОКИРУЮЩИМ). risk: medium.
- **EP-3** UI-панель эпизодов + обратимость (dismiss/restore/purge). risk: low. **ОБЯЗАТЕЛЬНО (контракт из adversarial-ревью EP-1, MAJOR-2):** команда переключения тоггла `episodic.enabled` в ON ДОЛЖНА сразу `enqueue` джобу `episode_rollup` (kick) — иначе фича «мертва до перезапуска vault»: seed на открытии гейтится `is_enabled` (зачем — zero-overhead когда OFF), а recurring-цепочка бутстрапится только из УСПЕШНОГО прогона. Без kick включение в работающем приложении ничего не запустит до следующего открытия vault. (Тот же класс «карточка висит пустой», что AIP-5/#63.)
- **EP-4** Калибровка порога + полнота eval + доки. risk: low.
- **EP-5** (owner-gated) Time-range режим + мост к фактам. risk: low.
- **EP-6** (owner-gated) Rollup-рефлексия (эпизоды-из-эпизодов). risk: high.

---

## 13. Точки переиспользования (по живому коду)

- `commands/chat_sessions.rs::chat_log_exchange` + `chat_log::set_title` — best-effort суммаризация мелкой моделью.
- `chat_log.rs::search_memory`/`resolve_memory_hits`/`MemoryHit`/`messages_missing_vectors` — форма ретривала, DTO-зеркало, backfill-паттерн `contains()`.
- `commands/chat.rs::ChatStreamEvent` (`MemorySources`) + `prepend_memory_block` — `EpisodeSources` + инъекция тем же путём.
- `ai/chat.rs::build_memory_block`/`build_agent_memory_block`/`injection_marker`/`truncate_chars`/`prepend_memory_block`.
- `commands/vault.rs::build_rag` (open vectors), `reconcile_embedding_model`, seed run-if-overdue, recurring map, chat_vectors-backfill.
- `digest/mod.rs::DigestHandler` — `JobHandler` не-стрим LLM + `defer_under_interactive` + recurring registration.
- `scheduler/mod.rs` (`run_due`/`reschedule_if_absent`/`has_ready_job`).
- `vector/mod.rs::VectorIndex` (`open`/`upsert`/`remove`/`save`/`search`/`contains`).
- `db/migrations.rs::Migration` — миграция 019.
- `eval/consolidation.rs` — образец `eval/episodes.rs`.
- `MemoryPanel`/`GoalsPanel` + `useFocusTrap` + `TRAP_OVERLAYS_CLOSED`; тоггл `aiAgentMemory`.
