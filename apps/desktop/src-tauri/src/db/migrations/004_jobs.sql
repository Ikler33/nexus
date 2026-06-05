-- Schema v4 (ADR-007): очередь фоновых задач планировщика. Источник — codesign egress+планировщик.
-- kind/payload-агностична; состояния pending → running → done | dead. Не трогает chunks/FTS → производных
-- для пересборки нет (rebuild_fts не нужен). Слой данных — slice 1; воркер-луп/триггеры/kind — далее.
CREATE TABLE jobs (
    id           INTEGER PRIMARY KEY,
    kind         TEXT    NOT NULL,                 -- тип задачи (агностично к движку)
    payload      TEXT    NOT NULL DEFAULT '',      -- JSON-параметры
    state        TEXT    NOT NULL DEFAULT 'pending', -- pending | running | done | dead
    run_at       INTEGER NOT NULL,                 -- unix-сек: не запускать раньше (расписание/backoff)
    attempts     INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 5,
    last_error   TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);

-- Выборка готовых к запуску (claim): WHERE state='pending' AND run_at<=now ORDER BY run_at.
CREATE INDEX idx_jobs_claim ON jobs(state, run_at);
