-- Schema v8 (HOME H2): кэш LLM-виджетов дашборда. Виджеты генерируются фоном (планировщик ADR-007) и
-- читаются мгновенно из этого кэша — LLM никогда не блокирует загрузку HOME (концепт PKM_Home_Concepts
-- §«Принципы»). `content` непрозрачен для кэша (текст/JSON — парсит конкретный виджет). Инвалидация по
-- правкам vault: `source_hash` = `max_file_mtime` на момент генерации; текущий mtime > source_hash ⇒ stale.
-- Новая таблица, производных для FTS не инвалидирует (rebuild_fts не нужен).
CREATE TABLE home_widgets (
    key          TEXT    PRIMARY KEY,   -- идентификатор виджета (daily_brief, stale_radar, …)
    content      TEXT    NOT NULL,      -- сгенерированное содержимое (текст/JSON, непрозрачно для кэша)
    generated_at INTEGER NOT NULL,      -- unix-сек последней успешной генерации
    source_hash  INTEGER NOT NULL,      -- max_file_mtime vault на момент генерации (инвалидация по правкам)
    status       TEXT    NOT NULL       -- 'ready' (контент валиден) | 'error' (последний refresh упал)
);
