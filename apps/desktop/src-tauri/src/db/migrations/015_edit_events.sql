-- 015: edit_events — честная ось времени (P2, мандат 5).
--
-- Журнал НАБЛЮДЁННЫХ изменений заметок: источник истины для временных фич (on-this-day, heatmap
-- активности, профиль пользователя) ВМЕСТО files.created_at (= mtime, врёт после git-clone/синка).
-- ts = когда Nexus УВИДЕЛ изменение (не реконструкция из mtime): на первом скане всё помечается
-- «сегодня» (честно — Nexus начал отслеживать сейчас), дальше каждое реальное сохранение даёт точку.
-- Пишется в той же транзакции index_file/remove_file ТОЛЬКО при реальной смене хеша (force-rescan и
-- повторная индексация неизменённого файла НЕ плодят фантомные события).
CREATE TABLE edit_events (
    id          INTEGER PRIMARY KEY,
    file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    ts          INTEGER NOT NULL,            -- unix-секунды наблюдения
    kind        TEXT    NOT NULL,            -- 'create' | 'modify' | 'delete'
    words_delta INTEGER NOT NULL DEFAULT 0,  -- изменение числа слов относительно прошлого события файла
    words_after INTEGER NOT NULL DEFAULT 0,  -- число слов после изменения (0 для delete)
    source      TEXT    NOT NULL DEFAULT 'app'
);

-- Запросы временной оси: «что менялось за период» (heatmap/on-this-day) и «история файла».
CREATE INDEX idx_edit_events_ts ON edit_events(ts);
CREATE INDEX idx_edit_events_file_ts ON edit_events(file_id, ts);
