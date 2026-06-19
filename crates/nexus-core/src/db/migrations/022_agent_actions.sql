-- Idempotency-ledger актуатора (AGENT-3b, Фаза 1): долговечная запись о КАЖДОМ действии актуатора
-- внутри прогона агента — write-before-act основа. AGENT-2 (021 agent_runs) фиксирует ПРОГОН целиком;
-- эта таблица — гранулярность ниже: одно действие = одна строка с классифицированным риском, состоянием
-- статус-машины, токеном оптимистичной конкуренции (content_hash на момент classify) и ИСХОДОМ.
--
-- Контракт идемпотентности (ключевой): replay ветвится по ПРИСУТСТВИЮ outcome, НЕ по присутствию ключа.
--   - ключа нет          → Fresh (свежее действие, исполнять);
--   - ключ есть, outcome NULL → CrashedMidExecute (упали между write-before и фиксацией исхода; вызывающий
--                               в AGENT-3c пере-проверит on-disk content_hash и решит повтор/пропуск);
--   - ключ есть, outcome NOT NULL → AlreadyDone (вернуть записанный исход, НЕ повторять).
-- Поэтому outcome НЕ имеет DEFAULT и стартует NULL: его ПРИСУТСТВИЕ — единственный признак терминальности
-- для replay (а не значение state — state может быть 'executing' и при краше, и до фиксации исхода).
--
-- idempotency_key = blake3(run_id, tool_name, canonical_args, target_hash@classify) — UNIQUE-фенс: два
-- идентичных действия в одном прогоне не порождают двух строк (INSERT OR ABORT на дубль → caller делает
-- lookup и ветвится по replay_decision). target_hash берётся НА МОМЕНТ classify (часть ключа), а
-- content_hash хранится отдельной колонкой как токен оптимистичной конкуренции для re-check в 3c.
--
-- Append/update-only by design: строки НЕ удаляются (как 020/021) — это журнал подотчётности действий
-- агента; меняется ТОЛЬКО state/outcome/undo/updated_at (переходы статус-машины + фиксация исхода).
--
-- FK на agent_runs НЕ ставим — по той же причине, что 021 не ставит FK на egress_audit: durable-журнал
-- не должен падать на отсутствующем/смешанном прогоне (записи могут появляться в нестандартных сценариях
-- восстановления/тестов); типы run_id совпадают (i64), корреляция — по значению.
CREATE TABLE agent_actions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          INTEGER NOT NULL,            -- = agent_runs.id (RunCtx.run_id); корреляция, без FK
    idempotency_key TEXT    NOT NULL UNIQUE,     -- blake3(run_id, tool_name, canonical_args, target_hash@classify)
    tool_name       TEXT    NOT NULL,            -- логическое имя инструмента (note_create|note_edit|frontmatter)
    target_rel      TEXT,                        -- vault-rel путь цели (NULL, если действие без файла)
    risk_tier       TEXT    NOT NULL,            -- auto|confirm|hardblocked (RiskTier::as_str)
    state           TEXT    NOT NULL,            -- classified|proposed|approved|rejected|executing|executed|failed|audited
    content_hash    TEXT,                        -- on-disk hash цели на момент classify (токен оптимистичной конкуренции для re-check в 3c)
    undo_kind       TEXT,                        -- дискриминант UndoHandle (snapshot|trash); NULL до исполнения
    undo_ref        TEXT,                        -- ссылка отката (snapshot ts / trash rel); NULL до исполнения
    outcome         TEXT,                        -- NULL до терминала; ПРИСУТСТВИЕ — ветка replay (НЕ значение)
    diff_summary    TEXT,                        -- усечённое резюме диффа (приватность; AGENT-6 ужесточит)
    created_at      INTEGER NOT NULL,            -- unix-сек вставки (write-before-act)
    updated_at      INTEGER NOT NULL             -- unix-сек последнего перехода
);
-- Выборка всех действий прогона (наблюдаемость/аудит-панель) — горячий путь чтения по run_id.
CREATE INDEX idx_agent_actions_run ON agent_actions(run_id);
