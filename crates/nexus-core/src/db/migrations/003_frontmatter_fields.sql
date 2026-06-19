-- 003: Типизированные поля frontmatter (V4.x typed-frontmatter).
-- Плоские скаляры верхнего уровня (progress/due/goal/evergreen/draft и пр.) как key→value —
-- для КРОСС-ФАЙЛОВЫХ запросов (цели/stale-radar/Dataview). Парсятся мини-парсером без YAML-либы
-- (serde_yaml архивирован → security-гейт; выбор владельца). Списки/вложенный YAML сюда НЕ попадают
-- (fallback — сырой блок в files.frontmatter); aliases/tags — в своих таблицах.
CREATE TABLE frontmatter_fields (
    id      INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    key     TEXT NOT NULL,
    value   TEXT NOT NULL,
    UNIQUE(file_id, key)
);

CREATE INDEX idx_frontmatter_fields_file ON frontmatter_fields(file_id);
-- Индекс по ключу — под запросы «все заметки с полем X» (цели/Dataview/stale-radar).
CREATE INDEX idx_frontmatter_fields_key ON frontmatter_fields(key);
