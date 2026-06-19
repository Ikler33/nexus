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
