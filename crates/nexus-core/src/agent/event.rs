//! `AgentEvent` — поток структурных событий цикла агента (AGENT-1).
//!
//! Цикл агента ОБЯЗАН эмитить не только финальную строку, а ПОТОК событий, который потребляет будущий
//! Agent UI (UI-1, см. `agent-ui-design/CONTRACT-NOTES.md`). Это контракт «бэкенд → фронт»: каждое
//! изменение состояния хода (токен ассистента, вызов инструмента, его результат, загрузка контекста,
//! финал/ошибка) становится отдельным событием. AGENT-1 покрывает МИНИМУМ из CONTRACT-NOTES §«поток
//! AgentEvent»; план/предложения/отчёт приходят более поздними срезами (см. ниже).

use serde::{Deserialize, Serialize};

/// Статус файла в changeset'е (CONTRACT-NOTES §«Changeset / предложения»: `status: new|edit`).
///
/// `New` — заметка создаётся (NoteCreate); `Edit` — существующая перезаписывается/правится
/// (NoteEdit/Frontmatter). Сериализуется как `"new"`/`"edit"` (camelCase) — точное соответствие
/// контракту дизайна, чтобы фронт мог различать иконку/семантику строки changeset'а.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    /// Новая заметка (create) — `status: "new"`.
    New,
    /// Правка существующей заметки (edit/frontmatter) — `status: "edit"`.
    Edit,
}

/// Один файл changeset'а в [`AgentEvent::Proposal`] — строка поверхности аппрува (CONTRACT-NOTES
/// §«Changeset / предложения»: `{path, add:int, del:int, status: new|edit}`). `action_id` — `id`
/// строки `agent_actions` (ledger) в состоянии `proposed`: им фронт/контрол-плейн адресует решение
/// (Approve/Reject) КОНКРЕТНОМУ предложенному действию (см. [`crate::actuator::decision::BatchDecision`]).
///
/// `state` (pending|applied|rejected из CONTRACT-NOTES) НЕ дублируется в событии: в момент эмиссии
/// Proposal все файлы по определению `pending` (батч только что записан в ledger); финальный
/// applied/rejected фронт получает из ledger-строки/последующих событий — событие Proposal несёт
/// changeset «как предложено», без преждевременного исхода.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedFile {
    /// vault-rel путь цели.
    pub path: String,
    /// Число добавленных строк (current → proposed; простой line-diff, AGENT-6 уточнит/усечёт).
    pub add: u32,
    /// Число удалённых строк (current → proposed).
    pub del: u32,
    /// new (create) | edit (overwrite/frontmatter).
    pub status: FileStatus,
    /// `id` строки `agent_actions` (state=proposed) — адрес решения Approve/Reject для этого файла.
    pub action_id: i64,
}

/// Статус ОДНОГО шага плана (SUB-2, ACP `AgentPlanUpdate`-семантика). ЗАКРЫТЫЙ набор (не free-form):
/// фронт рисует иконку по дискриминанту. Сериализуется camelCase (`pending`/`running`/`done`/`failed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanStepState {
    /// Ещё не начат.
    Pending,
    /// Выполняется.
    Running,
    /// Завершён успешно.
    Done,
    /// Провалился.
    Failed,
}

/// Один шаг плана (SUB-2): стабильный `id` (адрес для [`AgentEvent::PlanStepStatus`]-обновлений),
/// человекочитаемый `label`, текущий `status`. Узел «графа плана» правого дока (deep-research/делегирование).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    /// Стабильный идентификатор шага (по нему адресуются обновления статуса).
    pub id: String,
    /// Человекочитаемая метка шага (под-вопрос/подзадача).
    pub label: String,
    /// Текущий статус шага.
    pub status: PlanStepState,
}

/// Статус СУБАГЕНТА в дереве делегирования (SUB-2). ЗАКРЫТЫЙ набор; жизненный цикл узла
/// `spawned → running → done | failed | paused`. Сериализуется camelCase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubagentState {
    /// Субагент создан (строка `agent_runs` заведена), но цикл ещё не пошёл.
    Spawned,
    /// Цикл субагента выполняется.
    Running,
    /// Субагент завершил задачу (вернул саммари).
    Done,
    /// Субагент упал/исчерпал бюджет.
    Failed,
    /// Субагент остановлен kill-switch'ем (пауза родителя).
    Paused,
}

/// Кап `goal` в [`AgentEvent::SubagentStatus`] (редакция: не льём огромную цель в стрим).
pub const SUBAGENT_GOAL_MAX_CHARS: usize = 200;
/// Кап `summary` субагента (компактный итог; сырой вывод ребёнка в стрим НЕ идёт — приватность).
pub const SUBAGENT_SUMMARY_MAX_CHARS: usize = 2000;

/// UTF-8-безопасная обрезка строки до `max` символов (+ маркер `…` при усечении).
fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Событие хода агента — единица потока, который цикл отдаёт через `on_event` (UI-1 потребитель).
///
/// `#[non_exhaustive]`: будущие срезы добавляют варианты (план/предложения/отчёт) БЕЗ слома match'ей
/// у вызывающих — обязательный `_ =>` рукав уже требуется. Сериализуется тегированно (`type` + payload),
/// чтобы фронт различал варианты по дискриминанту, как и было в референс-UI.
///
/// # Маппинг на `CONTRACT-NOTES.md` (§«поток AgentEvent»)
/// - [`AgentEvent::AssistantToken`] ← `AssistantToken(String)` — контент модели стримом.
/// - [`AgentEvent::ToolCall`] ← `ToolCall { kind, args }` — перед исполнением инструмента (+ `id`
///   для корреляции с результатом — лента шагов разворачивает tool-действие по этой паре).
/// - [`AgentEvent::ToolResult`] ← `ToolResult { call_id, content, is_error }` — после исполнения.
/// - [`AgentEvent::ContextUsage`] ← `ContextUsage { used, window }` — из `ContextBudget` (P0-c),
///   питает %-бар «used/window токенов» в шапке сессии.
/// - [`AgentEvent::Final`] / [`AgentEvent::Error`] ← `Final(String)` / `Error(String)`.
///
/// # Что придёт ПОЗЖЕ (спроектировано как точки расширения, см. `#[non_exhaustive]`)
/// CONTRACT-NOTES перечисляет варианты последующих срезов — здесь НЕ реализованы (стабы шлют только
/// ToolCall/ToolResult), но enum намеренно открыт под них:
/// - `PlanProposed(steps)` / `PlanStepStatus(id, status)` — упорядоченные шаги плана + их статусы
///   (правый док + инлайн-лента) — **AGENT-2** (план).
/// - [`AgentEvent::Proposal`]/[`AgentEvent::Diff`] — поверхность changeset/аппрува актуатора
///   (файлы new|edit, +/−) — **AGENT-3d** (актуатор/автономность) реализует их здесь; аппрув-UX UI-1.
/// - `Report(doc)` — сгенерированный отчётный документ (правый док: title/meta/summary/bullets) —
///   research-задачи.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum AgentEvent {
    /// Дельта контента ассистента (стрим токенов модели) — живой вывод в ленте.
    AssistantToken(String),
    /// Намерение вызвать инструмент ДО исполнения. `id` коррелирует с [`AgentEvent::ToolResult`];
    /// `kind` — дотированное имя инструмента (напр. `"fs.read"`, `"debug.echo"`); `args` — сырой
    /// JSON-аргумент (как вернула модель, НЕ пере-десериализованный — fail-closed на границе).
    ToolCall {
        id: String,
        kind: String,
        args: String,
    },
    /// Результат исполнения инструмента. `id` == `id` соответствующего [`AgentEvent::ToolCall`];
    /// `content` — текст результата (УЖЕ зафенсенный при ре-инъекции в промпт, см. цикл); `is_error`
    /// — инструмент вернул ошибку (модель может восстановиться, цикл не падает).
    ToolResult {
        id: String,
        content: String,
        is_error: bool,
    },
    /// Загрузка контекстного окна модели в токенах: `used` из `ContextBudget`/токенайзера, `window`
    /// — полное окно модели. Питает %-бар «used/window» (CONTRACT-NOTES §«Сессия / run»).
    ContextUsage { used: usize, window: usize },
    /// Changeset, ожидающий решения (confirm-run / Confirm-тир) ЛИБО о котором уведомляют перед
    /// авто-применением (auto-run). `run_id` коррелирует с прогоном; `files` — строки поверхности
    /// аппрува (CONTRACT-NOTES §«Changeset / предложения»). К моменту эмиссии каждая строка `files`
    /// уже записана в ledger как `proposed` (её `action_id` адресует решение). Это «N pending · агент
    /// ждёт» из дизайна. Эмитится ОДИН раз на батч; индивидуальные дифы дублируются [`AgentEvent::Diff`].
    Proposal {
        // `rename_all` контейнера НЕ каскадирует в поля struct-вариантов enum (serde-семантика),
        // поэтому camelCase для составного имени задаём явно — фронт получает `runId`, как ProposedFile
        // получает `actionId` (там каскад работает: standalone-struct). Однословные поля (add/del/path/
        // status) одинаковы в обоих регистрах.
        #[serde(rename = "runId")]
        run_id: i64,
        files: Vec<ProposedFile>,
    },
    /// Пер-файловый диф changeset'а (CONTRACT-NOTES §«Лента шагов»: `{path, add, del, status}`).
    /// Эмитится ПОСЛЕ соответствующего [`AgentEvent::Proposal`] — по одному на файл, чтобы лента
    /// шагов разворачивала диф инлайн рядом с tool-действием. `add`/`del` — простой line-diff
    /// (current → proposed); хунки/усечение — AGENT-6.
    Diff {
        path: String,
        add: u32,
        del: u32,
        status: FileStatus,
    },
    /// Финальный ответ агента (модель завершила ход без новых tool_call).
    Final(String),
    /// Терминальная ошибка хода (исчерпан бюджет инициации стрима / провайдер упал и т.п.). Ошибки
    /// ОТДЕЛЬНЫХ инструментов сюда НЕ идут — они возвращаются как [`AgentEvent::ToolResult`]`{is_error}`.
    Error(String),
    /// Exec-предложение (Фаза-3 SANDBOX-6c): агент предлагает ИСПОЛНИТЬ команду В ПЕСОЧНИЦЕ
    /// (shell/process/git), ожидающее host-решения (Confirm-тир; exec НИКОГДА не Auto). `run_id`
    /// коррелирует с прогоном; `action_id` — `id` строки `agent_actions` (state=proposed) для адресации
    /// решения; `summary` — РЕДАКЦИЯ-БЕЗОПАСНЫЙ силуэт (имя инструмента/`op`-токен + счётчики argv), НЕ сырые
    /// argv-значения/env (приватность §5.6 — зеркало [`ProposedFile`]/diff-дисциплины). Эмитится host-side
    /// в `dispatch_exec_decision` ДО запроса решения. **STRUCT-вариант** (не newtype): serde-internal-tag
    /// (`#[serde(tag="type")]`) НЕ сериализует newtype-варианты → они молча терялись бы на проводе.
    ExecProposal {
        // `rename_all` контейнера НЕ каскадирует в поля struct-вариантов enum (serde-семантика) — задаём
        // camelCase для составных имён явно (как `Proposal.runId`); фронт получает `runId`/`actionId`.
        #[serde(rename = "runId")]
        run_id: i64,
        #[serde(rename = "actionId")]
        action_id: i64,
        summary: String,
    },
    /// Результат исполненного exec (после report-фазы 6c-2): `exit_code` — код возврата процесса;
    /// `finalized` — ledger переведён в терминальное executed|failed. `run_id`/`action_id` коррелируют с
    /// [`AgentEvent::ExecProposal`]. **СОДЕРЖИМОЕ-СВОБОДЕН by-design**: сырой stdout/stderr сюда НЕ кладётся
    /// (приватность §5.6 — вывод видит лишь модель через fenced tool-result). **STRUCT-вариант** (не newtype).
    ExecResult {
        #[serde(rename = "runId")]
        run_id: i64,
        #[serde(rename = "actionId")]
        action_id: i64,
        #[serde(rename = "exitCode")]
        exit_code: i32,
        finalized: bool,
    },
    /// **SUB-2: предложенный ПЛАН** (упорядоченные шаги) — «граф плана» правого дока (deep-research
    /// decompose / делегирование). `run_id` коррелирует с прогоном; `steps` — закрытый список
    /// [`PlanStep`]. Эмиттер придёт позже (RES-1/SUB-3) — SUB-2 закладывает только контракт.
    PlanProposed {
        #[serde(rename = "runId")]
        run_id: i64,
        steps: Vec<PlanStep>,
    },
    /// **SUB-2: обновление статуса ОДНОГО шага плана** (по стабильному `id` из [`PlanStep`]). Инлайн-
    /// лента/правый док перерисовывают шаг. ACP `AgentPlanUpdate`-семантика.
    PlanStepStatus { id: String, status: PlanStepState },
    /// **SUB-2: статус СУБАГЕНТА** в дереве делегирования (узел плана-графа по `parent_run_id` lineage).
    /// `summary` — РЕДАКЦИЯ-БЕЗОПАСНЫЙ итог (НЕ сырой вывод/рассуждения ребёнка; приватность — зеркало
    /// силуэт-дисциплины [`ProposedFile`]/[`AgentEvent::ExecProposal`]). Строить ТОЛЬКО через
    /// [`AgentEvent::subagent_status`] (клип `goal`/`summary`). `parent_run_id`/`child_run_id`/`summary`
    /// — явный camelCase (rename_all не каскадирует в struct-варианты enum).
    SubagentStatus {
        #[serde(rename = "parentRunId")]
        parent_run_id: i64,
        #[serde(rename = "childRunId")]
        child_run_id: i64,
        goal: String,
        status: SubagentState,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// **RES-5: сгенерированный отчёт deep-research** (правый док / лента документов). Эмитится ПОСЛЕ
    /// успешной записи заметки `research.run` через гейт. `path` — vault-rel путь; `title` (клип) — для
    /// карточки; `sources_count`/`rounds` — мета. Реализует зарезервированный `Report(doc)` контракт.
    /// Поля — явный camelCase (rename_all не каскадирует в struct-варианты enum).
    Report {
        #[serde(rename = "runId")]
        run_id: i64,
        title: String,
        path: String,
        #[serde(rename = "sourcesCount")]
        sources_count: usize,
        rounds: usize,
    },
}

/// Кап заголовка в [`AgentEvent::Report`] (карточка дока, не льём простыню).
pub const REPORT_TITLE_MAX_CHARS: usize = 200;

impl AgentEvent {
    /// Конструктор [`AgentEvent::Report`] с клипом `title` ([`REPORT_TITLE_MAX_CHARS`]). Эмиттер (RES-4/5)
    /// строит событие ТОЛЬКО так.
    pub fn report(
        run_id: i64,
        title: &str,
        path: &str,
        sources_count: usize,
        rounds: usize,
    ) -> Self {
        AgentEvent::Report {
            run_id,
            title: clip_chars(title, REPORT_TITLE_MAX_CHARS),
            path: path.to_string(),
            sources_count,
            rounds,
        }
    }
}

impl AgentEvent {
    /// Конструктор [`AgentEvent::SubagentStatus`] с РЕДАКЦИЕЙ: `goal`/`summary` клипуются
    /// ([`SUBAGENT_GOAL_MAX_CHARS`]/[`SUBAGENT_SUMMARY_MAX_CHARS`]). Эмиттер (SUB-3) обязан строить
    /// событие ТОЛЬКО так — не сырым литералом — чтобы в стрим не утёк длинный goal/итог.
    pub fn subagent_status(
        parent_run_id: i64,
        child_run_id: i64,
        goal: &str,
        status: SubagentState,
        summary: Option<&str>,
    ) -> Self {
        AgentEvent::SubagentStatus {
            parent_run_id,
            child_run_id,
            goal: clip_chars(goal, SUBAGENT_GOAL_MAX_CHARS),
            status,
            summary: summary.map(|s| clip_chars(s, SUBAGENT_SUMMARY_MAX_CHARS)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FileStatus` сериализуется как `"new"`/`"edit"` — точное соответствие CONTRACT-NOTES
    /// (`status: new|edit`).
    #[test]
    fn file_status_serializes_new_edit() {
        assert_eq!(serde_json::to_string(&FileStatus::New).unwrap(), "\"new\"");
        assert_eq!(
            serde_json::to_string(&FileStatus::Edit).unwrap(),
            "\"edit\""
        );
    }

    /// `AgentEvent::Proposal` сериализуется тегированно (`type:"proposal"`) с camelCase-полями файла
    /// {path, add, del, status, actionId} — поверхность changeset'а CONTRACT-NOTES.
    #[test]
    fn proposal_event_shape_matches_contract() {
        let ev = AgentEvent::Proposal {
            run_id: 7,
            files: vec![ProposedFile {
                path: "Notes/N.md".into(),
                add: 3,
                del: 1,
                status: FileStatus::Edit,
                action_id: 42,
            }],
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "proposal");
        assert_eq!(v["runId"], 7);
        let f = &v["files"][0];
        assert_eq!(f["path"], "Notes/N.md");
        assert_eq!(f["add"], 3);
        assert_eq!(f["del"], 1);
        assert_eq!(f["status"], "edit");
        assert_eq!(f["actionId"], 42);
        // round-trip (Deserialize тоже есть на enum).
        let back: AgentEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back, ev);
    }

    /// `AgentEvent::Diff` сериализуется как `type:"diff"` + {path, add, del, status} — CONTRACT-NOTES
    /// диф-блок ленты шагов.
    #[test]
    fn diff_event_shape_matches_contract() {
        let ev = AgentEvent::Diff {
            path: "New.md".into(),
            add: 5,
            del: 0,
            status: FileStatus::New,
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "diff");
        assert_eq!(v["path"], "New.md");
        assert_eq!(v["add"], 5);
        assert_eq!(v["del"], 0);
        assert_eq!(v["status"], "new");
    }

    /// SUB-2: `PlanProposed` сериализуется тегированно (`type:"planProposed"`, `runId`) со step.status
    /// camelCase; round-trip сохраняет всё.
    #[test]
    fn plan_proposed_serializes_tagged_camelcase() {
        let ev = AgentEvent::PlanProposed {
            run_id: 5,
            steps: vec![
                PlanStep {
                    id: "q1".into(),
                    label: "подвопрос 1".into(),
                    status: PlanStepState::Pending,
                },
                PlanStep {
                    id: "q2".into(),
                    label: "подвопрос 2".into(),
                    status: PlanStepState::Running,
                },
            ],
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "planProposed");
        assert_eq!(v["runId"], 5);
        assert_eq!(v["steps"][0]["id"], "q1");
        assert_eq!(v["steps"][0]["status"], "pending");
        assert_eq!(v["steps"][1]["status"], "running");
        let back: AgentEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back, ev);
    }

    /// SUB-2: `PlanStepStatus` — `type:"planStepStatus"` + {id, status}; статус — закрытый camelCase.
    #[test]
    fn plan_step_status_serializes() {
        let ev = AgentEvent::PlanStepStatus {
            id: "q1".into(),
            status: PlanStepState::Done,
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "planStepStatus");
        assert_eq!(v["id"], "q1");
        assert_eq!(v["status"], "done");
    }

    /// SUB-2: `subagent_status` КОНСТРУКТОР клипует goal/summary (редакция-безопасность — сырой длинный
    /// итог ребёнка в стрим не уходит). camelCase составных имён (parentRunId/childRunId); None-summary
    /// опускается.
    #[test]
    fn subagent_status_summary_is_capped_redaction_safe() {
        let huge_goal = "g".repeat(SUBAGENT_GOAL_MAX_CHARS + 100);
        let huge_summary = "s".repeat(SUBAGENT_SUMMARY_MAX_CHARS + 500);
        let ev =
            AgentEvent::subagent_status(3, 7, &huge_goal, SubagentState::Done, Some(&huge_summary));
        match &ev {
            AgentEvent::SubagentStatus {
                goal,
                summary,
                parent_run_id,
                child_run_id,
                status,
            } => {
                assert_eq!(*parent_run_id, 3);
                assert_eq!(*child_run_id, 7);
                assert_eq!(*status, SubagentState::Done);
                assert!(
                    goal.chars().count() <= SUBAGENT_GOAL_MAX_CHARS + 1,
                    "goal клипнут (+1 на маркер …)"
                );
                assert!(goal.ends_with('…'), "маркер усечения goal");
                let s = summary.as_ref().unwrap();
                assert!(
                    s.chars().count() <= SUBAGENT_SUMMARY_MAX_CHARS + 1,
                    "summary клипнут"
                );
                assert!(s.ends_with('…'));
            }
            _ => panic!("ожидался SubagentStatus"),
        }
        // Сериализация: camelCase составных имён + None-summary опускается.
        let short = AgentEvent::subagent_status(1, 2, "цель", SubagentState::Spawned, None);
        let v: serde_json::Value = serde_json::to_value(&short).unwrap();
        assert_eq!(v["type"], "subagentStatus");
        assert_eq!(v["parentRunId"], 1);
        assert_eq!(v["childRunId"], 2);
        assert_eq!(v["status"], "spawned");
        assert!(
            v.get("summary").is_none(),
            "None-summary опущен (skip_serializing_if)"
        );
        // round-trip.
        let back: AgentEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back, short);
    }

    /// `#[non_exhaustive]` сохранён: ВНЕШНИЙ (out-of-crate) match ОБЯЗАН иметь `_ =>` рукав, и
    /// существующие явные рукава (включая старые AGENT-1 варианты) компилируются рядом с новыми
    /// Proposal/Diff. В ЭТОМ крейте `_` локально недостижим (все варианты видимы) — `#[allow(
    /// unreachable_patterns)]` намеренно держит рукав, документируя ВНЕшний контракт non_exhaustive.
    #[test]
    fn non_exhaustive_match_still_compiles() {
        fn describe(ev: &AgentEvent) -> &'static str {
            #[allow(unreachable_patterns)]
            // out-of-crate этот `_` обязателен (non_exhaustive); тут — документ.
            match ev {
                AgentEvent::AssistantToken(_) => "token",
                AgentEvent::ToolCall { .. } => "call",
                AgentEvent::ToolResult { .. } => "result",
                AgentEvent::ContextUsage { .. } => "usage",
                AgentEvent::Proposal { .. } => "proposal",
                AgentEvent::Diff { .. } => "diff",
                AgentEvent::Final(_) => "final",
                AgentEvent::Error(_) => "error",
                // Обязательный (вне крейта) catch-all: будущие варианты не сломают внешний match.
                _ => "unknown",
            }
        }
        assert_eq!(
            describe(&AgentEvent::Diff {
                path: "x".into(),
                add: 0,
                del: 0,
                status: FileStatus::New
            }),
            "diff"
        );
        assert_eq!(
            describe(&AgentEvent::Final("done".into())),
            "final",
            "старые варианты компилируются рядом с новыми"
        );
    }
}
