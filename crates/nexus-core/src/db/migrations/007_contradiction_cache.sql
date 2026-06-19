-- Schema v7 (CT-3): кэш вердиктов «Поиска противоречий». Чтобы при частых прогонах (recurring раз/сутки
-- + on-change по правкам, ADR-007 slice 6/7) НЕ пере-судить LLM-ом неизменённые пары. Ключ — пара путей;
-- хэши — от тех же сниппетов, что видит судья (изменился сниппет → хэш другой → пере-судим). Производных
-- для FTS нет (rebuild_fts не нужен).
CREATE TABLE contradiction_cache (
    path_a        TEXT    NOT NULL,
    path_b        TEXT    NOT NULL,
    hash_a        INTEGER NOT NULL,   -- хэш сниппета A (вход судьи)
    hash_b        INTEGER NOT NULL,   -- хэш сниппета B
    contradiction INTEGER NOT NULL,   -- 0/1 — вердикт (кэшируем и «нет», чтобы не пере-судить)
    ctype         TEXT    NOT NULL,
    explanation   TEXT    NOT NULL,
    judged_at     INTEGER NOT NULL,
    PRIMARY KEY (path_a, path_b)
);
