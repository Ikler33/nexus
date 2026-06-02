-- Schema v2 (Ф1-1). Источник: ARCHITECTURE §5. RAG-чанки + полнотекстовый поиск по телу.
-- usearch (векторный ANN) — отдельный sibling-файл .nexus/vectors.usearch, не в SQLite (Ф1-4).

-- Чанки для RAG (эмбеддинг 1:1 к чанкам — §6.1)
CREATE TABLE chunks (
    id           INTEGER PRIMARY KEY,
    file_id      INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    chunk_index  INTEGER NOT NULL,
    content      TEXT NOT NULL,
    char_start   INTEGER NOT NULL,
    char_end     INTEGER NOT NULL,
    heading_path TEXT,                  -- H1 > H2 > H3 путь к чанку
    token_count  INTEGER NOT NULL
);

CREATE INDEX idx_chunks_file ON chunks(file_id);

-- FTS5 поверх chunks.content (external-content): полнотекстовый поиск по ТЕЛУ.
-- В files нет колонки content, поэтому FTS строится на chunks (§5).
CREATE VIRTUAL TABLE fts_chunks USING fts5(
    content,
    content=chunks,
    content_rowid=id
);

-- ОБЯЗАТЕЛЬНЫЕ триггеры синхронизации external-content FTS (без них — рассинхрон):
CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
  INSERT INTO fts_chunks(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
  INSERT INTO fts_chunks(fts_chunks, rowid, content) VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER chunks_au AFTER UPDATE ON chunks BEGIN
  INSERT INTO fts_chunks(fts_chunks, rowid, content) VALUES ('delete', old.id, old.content);
  INSERT INTO fts_chunks(rowid, content)               VALUES (new.id, new.content);
END;
