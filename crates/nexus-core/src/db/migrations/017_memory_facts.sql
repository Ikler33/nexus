-- Schema v17 (MEM, vision-фича «персистентная память агента»): слой ЯВНЫХ ФАКТОВ о пользователе/
-- проектах, отдельный от RAG-по-переписке (N4b/chat_vectors). Факты курирует пользователь (D1/D4),
-- инжектятся в контекст ответа ИИ: пины «всегда» + top-k семантически близких (D2). Эмбеддинги — в
-- параллельном usearch-индексе `memory_vectors` (ключ = memory_facts.id), как chat_vectors. Спека:
-- docs/specs/agent-memory.md. Фича ВЫКЛ по умолчанию (D5). Производных для FTS нет (rebuild_fts=false).
CREATE TABLE memory_facts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    text        TEXT    NOT NULL,           -- сам факт (вход эмбеддинга)
    pinned      INTEGER NOT NULL DEFAULT 0, -- 1 = всегда в контексте, не вытесняется (D2/D6)
    source      TEXT    NOT NULL,           -- 'explicit' | 'auto' (D1)
    created_at  INTEGER NOT NULL,           -- unix-секунды добавления
    used_at     INTEGER NOT NULL DEFAULT 0  -- последний раз подмешан в контекст (подсветка старых, D6)
);

-- Дедуп по точному тексту (AC-MEM-1) — не плодим одинаковые факты.
CREATE UNIQUE INDEX idx_memory_facts_text ON memory_facts(text);
