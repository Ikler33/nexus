//! `AgentEvent` — поток структурных событий цикла агента (AGENT-1).
//!
//! Цикл агента ОБЯЗАН эмитить не только финальную строку, а ПОТОК событий, который потребляет будущий
//! Agent UI (UI-1, см. `agent-ui-design/CONTRACT-NOTES.md`). Это контракт «бэкенд → фронт»: каждое
//! изменение состояния хода (токен ассистента, вызов инструмента, его результат, загрузка контекста,
//! финал/ошибка) становится отдельным событием. AGENT-1 покрывает МИНИМУМ из CONTRACT-NOTES §«поток
//! AgentEvent»; план/предложения/отчёт приходят более поздними срезами (см. ниже).

use serde::{Deserialize, Serialize};

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
/// - `Proposal`/`Diff { file, hunks, add, del, status }` — поверхность changeset/аппрува актуатора
///   (файлы new|edit, +/−) — **AGENT-3** (актуатор) + **AGENT-5** (аппрув/автономность).
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
    /// Финальный ответ агента (модель завершила ход без новых tool_call).
    Final(String),
    /// Терминальная ошибка хода (исчерпан бюджет инициации стрима / провайдер упал и т.п.). Ошибки
    /// ОТДЕЛЬНЫХ инструментов сюда НЕ идут — они возвращаются как [`AgentEvent::ToolResult`]`{is_error}`.
    Error(String),
}
