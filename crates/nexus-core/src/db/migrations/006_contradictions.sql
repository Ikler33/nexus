-- Schema v6 (#vision): «Поиск противоречий» — фоновый LLM-kind планировщика (ADR-007, спека
-- docs/specs/contradictions.md). Найденные пары конфликтующих/устаревших заметок. Прогон ЗАМЕНЯЕТ
-- предыдущий результат (CT-1 без кэша). Не трогает chunks/FTS → пересобирать нечего (rebuild_fts не нужен).
CREATE TABLE contradictions (
    id          INTEGER PRIMARY KEY,
    path_a      TEXT    NOT NULL,   -- путь первой заметки (a < b лексикографически)
    path_b      TEXT    NOT NULL,   -- путь второй заметки
    ctype       TEXT    NOT NULL,   -- 'hard' | 'soft' | 'temporal' (D3)
    explanation TEXT    NOT NULL,   -- краткое объяснение от LLM
    created_at  INTEGER NOT NULL    -- unix-сек прогона
);

CREATE INDEX idx_contradictions_created ON contradictions(created_at);
