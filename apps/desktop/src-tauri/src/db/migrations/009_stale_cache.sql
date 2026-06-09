-- Schema v9 (HOME H4): кэш LLM-обогащения «Stale radar» (зона 4 концепта). Слой 1 (скоринг устаревания)
-- считается на лету из метаданных индекса — кэш НЕ нужен. Слой 2 (LLM читает топ-N устаревших заметок →
-- причина/действие/подсказка) кэшируется здесь: ключ — путь; инвалидация по правкам файла
-- (`source_mtime` = `files.updated_at` на момент обогащения; изменился → пере-обогащаем) + TTL 24ч
-- (`generated_at`). Производных для FTS нет (rebuild_fts не нужен).
CREATE TABLE stale_cache (
    path         TEXT    PRIMARY KEY,   -- путь заметки (топ-N устаревших)
    source_mtime INTEGER NOT NULL,      -- files.updated_at на момент обогащения (инвалидация по правкам)
    reason       TEXT    NOT NULL,      -- одно предложение «почему устарело» (LLM)
    action       TEXT    NOT NULL,      -- рекомендованное действие: update | archive | split | delete
    hint         TEXT    NOT NULL,      -- конкретная подсказка (LLM)
    generated_at INTEGER NOT NULL       -- unix-сек обогащения (TTL 24ч)
);
