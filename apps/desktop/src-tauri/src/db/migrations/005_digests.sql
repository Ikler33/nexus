-- Schema v5 (#35): «Дайджест изменений» — первый LLM-kind планировщика (ADR-007). История
-- сгенерированных дайджестов. Не трогает chunks/FTS → производных для пересборки нет (rebuild_fts не нужен).
CREATE TABLE digests (
    id          INTEGER PRIMARY KEY,
    created_at  INTEGER NOT NULL,   -- unix-сек генерации
    since       INTEGER NOT NULL,   -- начало окна (какие изменения суммировались)
    content     TEXT    NOT NULL,
    note_count  INTEGER NOT NULL    -- сколько заметок вошло в дайджест
);

CREATE INDEX idx_digests_created ON digests(created_at);
