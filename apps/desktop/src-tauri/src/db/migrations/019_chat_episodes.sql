-- Эпизодическая память (EP): эпизод = саммари ОДНОЙ чат-сессии. 1:1 с chat_sessions (session_id UNIQUE).
-- Все поля ПРОИЗВОДНЫ от chat_messages → таблицу можно дропнуть/пересобрать без потери первоисточника
-- (rollup-джоба пере-сгенерирует эпизоды; сессии и сообщения целы).
-- ON DELETE CASCADE здесь — лишь корректность на случай БУДУЩЕГО удаления сессии; ОСНОВНОЙ путь полного
-- удаления эпизода — ЯВНАЯ команда episode_purge (в коде НЕТ delete-session; решение владельца «храним всё»).
-- Не полагаться на каскад как на GC.
CREATE TABLE chat_episodes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    INTEGER NOT NULL UNIQUE REFERENCES chat_sessions(id) ON DELETE CASCADE,
    summary       TEXT    NOT NULL,            -- связное саммари (RU): вход эмбеддинга + UI/инъекция
    topics        TEXT,                        -- JSON-массив строк-тем; NULL до заполнения
    msg_count     INTEGER NOT NULL,            -- покрытых сообщений (idempotency: пересжимаем при росте)
    last_msg_id   INTEGER NOT NULL,            -- max(chat_messages.id) на момент генерации — водяной знак
    started_at    INTEGER NOT NULL,            -- min(created_at) сессии — time-range ретривал
    ended_at      INTEGER NOT NULL,            -- max(created_at) сессии — time-range ретривал
    model         TEXT,                        -- chat_util|chat_fast — аудит/рекалибровка
    embed_model   TEXT,                        -- модель эмбеддинга summary (реконсиляция при смене)
    generated_at  INTEGER NOT NULL,
    dismissed     INTEGER NOT NULL DEFAULT 0   -- мягкое скрытие (обратимо); НЕ сбрасывается пересжатием
);
CREATE INDEX idx_chat_episodes_ended   ON chat_episodes(ended_at DESC);
CREATE INDEX idx_chat_episodes_session ON chat_episodes(session_id);
CREATE INDEX idx_chat_episodes_live    ON chat_episodes(dismissed, ended_at DESC);
-- Семантический индекс — НЕ в SQLite: .nexus/episode_vectors.usearch (ключ = chat_episodes.id),
-- открывается рядом с chat_vectors/memory_vectors в commands/vault.rs::build_rag.
