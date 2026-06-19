-- Schema v16 (AIP-10): кэш LLM-объяснений связи между двумя заметками («Связи»/«Похожие»).
-- Лениво, по видимой карточке (фронт), кэш — чтобы при повторном показе/смене активной заметки НЕ
-- дёргать LLM по неизменённым парам. ЗЕРКАЛО contradiction_cache (007): ключ — упорядоченная пара
-- путей (a<b нормализуется в КОДЕ, не в БД); hash_a/hash_b — от тех же сниппетов, что идут в prompt
-- (изменился первый чанк заметки → хэш другой → пере-генерим). Сниппеты НЕ храним (дёшево пересчитать
-- из chunks через note_snippet) — храним только результат LLM. Производных для FTS нет (rebuild_fts=false).
CREATE TABLE relation_reasons (
    path_a       TEXT    NOT NULL,
    path_b       TEXT    NOT NULL,
    hash_a       INTEGER NOT NULL,   -- хэш сниппета заметки A (вход LLM)
    hash_b       INTEGER NOT NULL,   -- хэш сниппета заметки B
    explanation  TEXT    NOT NULL,   -- объяснение связи от chat_util (кэшируем и пустую строку — не пере-генерить мусор)
    generated_at INTEGER NOT NULL,   -- unix-секунды генерации (лог; задел под TTL-GC)
    PRIMARY KEY (path_a, path_b)
);
