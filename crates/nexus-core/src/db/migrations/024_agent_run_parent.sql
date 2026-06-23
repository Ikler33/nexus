-- SUBAGENTS (SUB-0, Фаза-«делегирование»): связь прогона-ребёнка с родителем для ДЕРЕВА делегирования.
-- Субагент = ВТОРОЙ ин-процесс вызов run_agent_session со своей строкой agent_runs; parent_run_id
-- хранит id родителя → реконструкция дерева, per-child корреляция egress/ledger, узлы плана ACP.
--
-- parent_run_id = agent_runs.id родителя; NULL = TOP-LEVEL прогон. ОБРАТНАЯ СОВМЕСТИМОСТЬ: все
-- существующие create_run НЕ перечисляют parent_run_id в INSERT → колонка получает NULL по дефолту,
-- поведение прежних прогонов не меняется. Самоссылка; формальный FK НЕ ставим — durable-журнал не
-- должен падать (как и в 021: egress пишется в pre-vault окне, строки журнала не удаляются).
--
-- Индекс по parent_run_id — горячий путь будущих запросов «дети прогона X» (дерево/наблюдаемость).
ALTER TABLE agent_runs ADD COLUMN parent_run_id INTEGER;
CREATE INDEX idx_agent_runs_parent ON agent_runs(parent_run_id);
