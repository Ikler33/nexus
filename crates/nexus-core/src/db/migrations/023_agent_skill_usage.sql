-- Телеметрия и lifecycle скиллов агента (SELF-LEARNING SL-1, порт hermes skill_usage.py/curator.py).
-- Долговечная замена JSON-sidecar'у Hermes на наш SQLite-идиом: единственный WriteActor даёт атомарность
-- и блокировку «бесплатно» (без fcntl/atomic-tempfile). Одна строка на скилл (PK = stable Skill.name).
--
-- РАЗВЯЗКА (порт Hermes дословно): телеметрия (use/view/save/patch + last_*_at) пишется для ВСЕХ скиллов
-- независимо от происхождения (чистая наблюдаемость); LIFECYCLE-мутаторы (state/pinned/archive) — NO-OP,
-- если created_by != 'agent' (vendor/user-скиллы неизменяемы для curator'а). Enforce на data-слое
-- (skills/usage.rs) + WHERE-клаузой curator'а — defense-in-depth.
--
-- created_by — load-bearing curation-гейт (зеркало hermes mark_agent_created): ставится при agent-origin
-- skill_save (SL-7). NULL/'vendor'/'user' → curator НЕ трогает. Провенанс живёт ЗДЕСЬ, не в Skill-struct
-- (тот парсится из неизменяемого SKILL.md).
--
-- FK НЕ ставим (скиллы — FS-resident SKILL.md, не строки БД; та же причина, что 020/021/022 ledgers):
-- orphan-строки (скилл удалён с диска) толерируются и подчищаются curator'ом (forget-pass, hermes forget()).
-- activity-якорь = max(last_used/viewed/saved/patched); created_at — fallback (никогда-не-юзанный
-- agent-скилл всё равно прунабелен по возрасту, hermes _idle_days), но из «активности» исключён.
CREATE TABLE agent_skill_usage (
    skill_name      TEXT    PRIMARY KEY,         -- стабильный id = Skill.name; одна строка на скилл
    use_count       INTEGER NOT NULL DEFAULT 0,  -- activate_skill (использование инструкции)
    view_count      INTEGER NOT NULL DEFAULT 0,  -- read_skill_resource (просмотр ресурса, hermes view)
    save_count      INTEGER NOT NULL DEFAULT 0,  -- skill_save (создание/перезапись агентом, SL-7)
    patch_count     INTEGER NOT NULL DEFAULT 0,  -- будущий skill_patch/curator-консолидация
    last_used_at    INTEGER,                     -- unix-сек; NULL пока не использован
    last_viewed_at  INTEGER,
    last_saved_at   INTEGER,
    last_patched_at INTEGER,
    created_at      INTEGER NOT NULL,            -- unix-сек первого касания (для age-fallback пруна)
    created_by      TEXT,                        -- 'agent'|'vendor'|'user'|NULL — curation-гейт (lifecycle ТОЛЬКО для 'agent')
    state           TEXT    NOT NULL DEFAULT 'active'
                            CHECK (state IN ('active', 'stale', 'archived')),  -- lifecycle (curator, НИКОГДА delete)
    pinned          INTEGER NOT NULL DEFAULT 0,  -- 0/1: закреплён (curator не архивирует, сортируется выше)
    archived_at     INTEGER                      -- unix-сек перевода в archived (обратимо); NULL пока active/stale
);

-- Скан curator'а/UI отчёта agent_created_report: ведущий фильтр — `created_by='agent'`, затем lifecycle-
-- state, затем по активности. Индекс ведёт `created_by` (селективный фильтр запроса), потом state,
-- потом last_used_at. На таблице из десятков-сотен скиллов это скорее корректность намерения, чем перф
-- (full scan тривиален), но колонки совпадают с реальным путём доступа (ревью SL-1: прежний
-- (state,last_used_at) не вёл фильтр запроса).
CREATE INDEX idx_skill_usage_created_by_state ON agent_skill_usage (created_by, state, last_used_at);
