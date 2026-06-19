-- Schema v1 (Ф0-2). Источник истины: ARCHITECTURE §5.
-- Подмножество для Фазы 0: файлы, ссылки, теги, алиасы, настройки + индексы.
-- chunks / fts_chunks / usearch / chat_* / link_suggestions — отдельными миграциями в Ф1+.

-- Файлы vault
CREATE TABLE files (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,   -- относительный путь от корня vault (нормализованный)
    hash        TEXT NOT NULL,          -- blake3 хэш контента
    title       TEXT,                   -- из frontmatter или первого H1
    created_at  INTEGER NOT NULL,       -- unix ts из frontmatter или fs
    updated_at  INTEGER NOT NULL,
    indexed_at  INTEGER NOT NULL,       -- когда последний раз индексировали
    size_bytes  INTEGER NOT NULL,
    word_count  INTEGER NOT NULL DEFAULT 0,
    frontmatter TEXT,                   -- JSON blob всего frontmatter
    is_deleted  INTEGER NOT NULL DEFAULT 0  -- soft delete
);

-- Исходящие ссылки (беклинки = запрос по idx_links_target, ADR-004)
CREATE TABLE links (
    id          INTEGER PRIMARY KEY,
    source_id   INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    target_id   INTEGER REFERENCES files(id) ON DELETE SET NULL,
    target_raw  TEXT NOT NULL,          -- оригинальный текст [[ссылки]]
    link_type   TEXT NOT NULL,          -- 'wikilink' | 'markdown' | 'embed'
    context     TEXT,                   -- ~100 символов вокруг ссылки
    line_number INTEGER
);

-- Теги (нормализованные, lowercase)
CREATE TABLE tags (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL UNIQUE
);

CREATE TABLE file_tags (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (file_id, tag_id)
);

-- Алиасы файлов (из frontmatter aliases: [...])
CREATE TABLE aliases (
    id      INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    alias   TEXT NOT NULL,
    UNIQUE(alias)
);

-- Настройки приложения/vault (JSON-значения)
CREATE TABLE settings (
    key     TEXT PRIMARY KEY,
    value   TEXT NOT NULL
);

-- Индексы
CREATE INDEX idx_links_source ON links(source_id);
CREATE INDEX idx_links_target ON links(target_id);
CREATE INDEX idx_file_tags_file ON file_tags(file_id);
CREATE INDEX idx_files_updated ON files(updated_at);
