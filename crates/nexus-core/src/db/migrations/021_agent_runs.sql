-- Durable agent-run record (AGENT-2, Фаза 1): долговечная запись о прогоне цикла агента.
-- AGENT-1 крутил цикл `run_agent_loop` ИН-ПРОЦЕСС (smoke); AGENT-2 делает прогон ДОЛГОВЕЧНОЙ
-- запланированной джобой. Эта таблица — источник истины состояния прогона: статус-машина
-- (queued→running→done|error|cancelled), шаг цикла (наблюдаемость/replay), и корреляция с
-- egress_audit.run_id (тот же i64 = id строки прогона) — каждый эгресс внутри прогона
-- атрибутируется на него (EgressAudit::set_run).
--
-- Append/update-only by design: строки НЕ удаляются (кроме owner-gated purge будущих срезов) —
-- это журнал прогонов агента. Меняется ТОЛЬКО статус/шаг/исход/метка (переходы статус-машины).
--
-- id = AUTOINCREMENT INTEGER = тот самый i64 run_id, на который ссылается egress_audit.run_id
-- (совпадение типов FK; формальный FK не ставим — egress_audit может писаться в pre-vault окне и
-- в смешанных сценариях, durable-журнал не должен падать на отсутствующем прогоне).
CREATE TABLE agent_runs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,  -- = i64 run_id (= egress_audit.run_id)
    session_id TEXT,                                -- связь с chat-эпизодом/сессией (nullable пока)
    task       TEXT    NOT NULL,                    -- пользовательский промпт прогона
    status     TEXT    NOT NULL,                    -- queued|running|done|error|cancelled
    model      TEXT,                                -- id использованной модели
    autonomy   TEXT,                                -- confirm|auto (per-run политика из UI-контракта; пока не enforce)
    outcome    TEXT,                                -- финальный ответ либо текст ошибки (NULL до терминала)
    step       INTEGER NOT NULL DEFAULT 0,          -- достигнутый шаг цикла (наблюдаемость/replay)
    created_at INTEGER NOT NULL,                    -- unix-сек создания (status='queued')
    updated_at INTEGER NOT NULL                     -- unix-сек последнего перехода (для TTL crash-recovery)
);
-- Индекс по статусу: клейм/восстановление прогонов ('running' с устаревшим updated_at → 'queued',
-- и выборки по терминальным статусам) — горячий путь requeue_stale_running и наблюдаемости.
CREATE INDEX idx_agent_runs_status ON agent_runs(status);
