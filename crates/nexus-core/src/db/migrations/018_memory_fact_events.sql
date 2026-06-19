-- 018: история/версии фактов памяти (MEM-7) — фундамент ОБРАТИМОСТИ под консолидацию (MEM-8).
--
-- Журнал изменений факта (правка/удаление/замещение/восстановление) для аудита «почему ИИ заменил
-- факт» и отката. Зеркало edit_events (015), но для memory_facts.
CREATE TABLE memory_fact_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id     INTEGER NOT NULL,           -- БЕЗ FK-CASCADE: аудит переживает физическое удаление факта
    event       TEXT    NOT NULL,           -- 'update' | 'delete' | 'supersede' | 'restore'
    old_text    TEXT,                       -- текст до операции (NULL — нечего показать)
    new_text    TEXT,                       -- текст после (NULL для delete)
    op_group    INTEGER,                    -- группа СОСТАВНОЙ операции (MEM-8: ADD нового + supersede старого
                                            --   откатываются ВМЕСТЕ); NULL = одиночная операция
    created_at  INTEGER NOT NULL            -- unix-секунды
);

-- «История факта» (панель) и группированный откат (MEM-8).
CREATE INDEX idx_memory_fact_events_fact ON memory_fact_events(fact_id, created_at);
CREATE INDEX idx_memory_fact_events_group ON memory_fact_events(op_group);

-- Soft-supersede (наполняется в MEM-8): факт, замещённый консолидацией, НЕ удаляется физически, а
-- помечается — убирается из ретривала/списка, но восстановим (откат). `superseded_by` = id заместившего
-- факта. ИНВАРИАНТ: факт ЖИВ ⟺ `superseded_by IS NULL`. В MEM-7 ничто не супридит — колонки дремлют,
-- а фильтр `WHERE superseded_by IS NULL` устанавливает инвариант заранее (тестируется ручной пометкой).
ALTER TABLE memory_facts ADD COLUMN superseded_by INTEGER;
ALTER TABLE memory_facts ADD COLUMN superseded_at INTEGER;
