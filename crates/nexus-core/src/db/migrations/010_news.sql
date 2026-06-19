-- Schema v10 (News Feed NF-3, спека docs/specs/news-feed.md D6): лента новостей.
-- news_items: оценённые LLM-этапом записи; дедуп между прогонами — url UNIQUE (AC-NF-4,
-- ON CONFLICT DO NOTHING: обновлённый title по тому же url НЕ перетирает прочитанность).
-- Ретенция 30 дней по fetched_at чистит и items, и runs (AC-NF-5); «навсегда» = «в заметку»
-- (заметка живёт в vault, от ретенции не зависит). Производных для FTS нет (rebuild_fts не нужен).
CREATE TABLE news_items (
    id           INTEGER PRIMARY KEY,
    source_id    TEXT    NOT NULL,      -- id источника из реестра/конфига
    url          TEXT    NOT NULL UNIQUE,
    title        TEXT    NOT NULL,      -- оригинальный заголовок (для «в заметку»/тултипа)
    title_ru     TEXT    NOT NULL,      -- RU-заголовок от LLM (для RU-источников = оригинал)
    summary_ru   TEXT    NOT NULL,      -- 1–2 предложения RU-резюме (LLM)
    topic        TEXT    NOT NULL,      -- тема-кластер для группировки ленты (D4)
    lang_ru      INTEGER NOT NULL DEFAULT 0,
    published_at INTEGER NOT NULL,      -- unix-сек публикации (0 — фид без даты)
    fetched_at   INTEGER NOT NULL,      -- unix-сек прогона (ретенция)
    read_at      INTEGER,               -- NULL = непрочитано (AC-NF-4: переживает повторные прогоны)
    hidden       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_news_items_published ON news_items(published_at DESC);
CREATE INDEX idx_news_items_fetched   ON news_items(fetched_at);

-- Прогоны: RU-сводка дня + статистика «N из M источников, K не разобрано LLM» (no silent caps,
-- AC-NF-1/3/10). UI читает последнюю запись.
CREATE TABLE news_runs (
    id            INTEGER PRIMARY KEY,
    run_at        INTEGER NOT NULL,
    digest_ru     TEXT    NOT NULL,      -- сводка дня ('' — нечего сводить)
    items_new     INTEGER NOT NULL,      -- новых записей в этом прогоне (после дедупа)
    sources_ok    INTEGER NOT NULL,
    sources_total INTEGER NOT NULL,
    llm_failed    INTEGER NOT NULL,      -- записей не разобрано LLM-этапом
    errors        TEXT    NOT NULL       -- JSON-массив строк «источник: ошибка» (видимые пропуски)
);
