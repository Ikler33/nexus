-- Сессии чата (решение владельца 2026-06-12): переписка — часть «второго мозга», храним ВСЁ
-- (ничего не удаляем; экспорт в заметку — отдельной кнопкой). Заголовок генерится мелкой
-- моделью по первому вопросу (плейсхолдер до того — обрезанный вопрос).
CREATE TABLE chat_sessions (
    id         INTEGER PRIMARY KEY,
    title      TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_chat_sessions_updated ON chat_sessions(updated_at DESC);

-- Сообщения: контент + источники (vault/web) как JSON-снапшот — восстанавливаем карточки
-- при загрузке сессии. role: user | assistant.
CREATE TABLE chat_messages (
    id           INTEGER PRIMARY KEY,
    session_id   INTEGER NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    role         TEXT NOT NULL,
    content      TEXT NOT NULL,
    sources_json TEXT,
    created_at   INTEGER NOT NULL
);
CREATE INDEX idx_chat_messages_session ON chat_messages(session_id, id);
