-- W-38: персист ходов агента для истории переписок (агент-сессии).
CREATE TABLE agent_turns (
  run_id         INTEGER PRIMARY KEY,         -- = agent_runs.id (один ход = один run)
  session_id     TEXT NOT NULL,               -- группировка ходов в переписку
  task           TEXT NOT NULL,               -- промпт пользователя
  assistant_text TEXT NOT NULL DEFAULT '',    -- склеенный ответ ассистента
  report         TEXT,                         -- финальный ответ (терминал)
  error_text     TEXT,                         -- текст ошибки (если error)
  status         TEXT NOT NULL,                -- done|error|cancelled
  created_at     INTEGER NOT NULL
);
CREATE INDEX idx_agent_turns_session ON agent_turns(session_id, created_at);
CREATE TABLE agent_turn_steps (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id    INTEGER NOT NULL,
  ord       INTEGER NOT NULL,                  -- порядок в ходе
  kind      TEXT NOT NULL,
  args      TEXT NOT NULL DEFAULT '',
  title     TEXT,
  result    TEXT,
  is_error  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_agent_turn_steps_run ON agent_turn_steps(run_id, ord);
