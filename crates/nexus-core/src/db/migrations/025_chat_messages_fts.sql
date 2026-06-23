-- Session-search (#58): полнотекстовый поиск по переписке. External-content FTS5 над
-- chat_messages.content — зеркало fts_chunks (002). Триггеры ОБЯЗАТЕЛЬНЫ (иначе рассинхрон индекса).
-- Бэкфилл существующих сообщений (миграция применяется к НЕпустой БД — chat_messages с v12).
CREATE VIRTUAL TABLE fts_chat_messages USING fts5(
    content,
    content=chat_messages,
    content_rowid=id
);

CREATE TRIGGER chat_messages_ai AFTER INSERT ON chat_messages BEGIN
  INSERT INTO fts_chat_messages(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER chat_messages_ad AFTER DELETE ON chat_messages BEGIN
  INSERT INTO fts_chat_messages(fts_chat_messages, rowid, content) VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER chat_messages_au AFTER UPDATE ON chat_messages BEGIN
  INSERT INTO fts_chat_messages(fts_chat_messages, rowid, content) VALUES ('delete', old.id, old.content);
  INSERT INTO fts_chat_messages(rowid, content)                    VALUES (new.id, new.content);
END;

-- ВНИМАНИЕ на будущее: chat_messages имеет ON DELETE CASCADE от chat_sessions (012). Каскадное
-- удаление НЕ дёргает AFTER DELETE-триггер, если PRAGMA recursive_triggers=OFF (дефолт SQLite) →
-- осиротит строки в этом FTS-индексе. Сейчас delete-session-пути НЕТ (переписку не удаляем). Когда он
-- появится: либо включить recursive_triggers, либо чистить fts_chat_messages вручную, либо вызвать
-- rebuild_chat_fts() (примитив ремонта, паритет с rebuild_fts для chunks).

-- Бэкфилл уже накопленной переписки (триггеры ловят только НОВЫЕ строки). СОЗНАТЕЛЬНО одним стейтментом
-- в транзакции миграции (как любой бэкфилл): переписка — это текст диалогов, объём скромный; чанкинг/
-- резюмируемость не нужны. При рассинхроне ремонт — rebuild_chat_fts() без сноса .nexus.
INSERT INTO fts_chat_messages(rowid, content) SELECT id, content FROM chat_messages;
