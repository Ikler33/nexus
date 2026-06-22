//! AGENT-CONNECT wire-DTO событий агента (P0b) — ЕДИНЫЙ источник истины контракта «бэкенд→клиент».
//!
//! `AgentEvent` (ядро) помечен `#[serde(tag="type")]`, но имеет newtype-варианты (`Final(String)` и
//! т.п.), которые serde-internal-tag сериализовать НЕ может (см. регрессию в [`super`]). Поэтому
//! события уходят на провод через этот **struct-вариантный** DTO (теговый camelCase, корректно
//! сериализуется/парсится). Один DTO для ОБОИХ потребителей: desktop UI-1a (`Channel<AgentStreamEvent>`)
//! и agentd-коннектор (`agent/event`-нотификация) — чтобы фронт и сервис не разъехались по контракту.

use serde::{Deserialize, Serialize};

use crate::agent::event::{AgentEvent, FileStatus};

/// Статус файла changeset'а для клиента — `"new"`|`"edit"` (зеркало [`FileStatus`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentFileStatus {
    /// Новая заметка (create).
    New,
    /// Правка существующей (overwrite/frontmatter).
    Edit,
}

impl From<FileStatus> for AgentFileStatus {
    fn from(s: FileStatus) -> Self {
        match s {
            FileStatus::New => AgentFileStatus::New,
            FileStatus::Edit => AgentFileStatus::Edit,
        }
    }
}

/// Один файл предложения для клиента (поверхность аппрува). Зеркало [`crate::agent::ProposedFile`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProposedFile {
    /// vault-rel путь цели.
    pub path: String,
    /// Добавлено строк (line-diff current → proposed).
    pub add: u32,
    /// Удалено строк.
    pub del: u32,
    /// new | edit.
    pub status: AgentFileStatus,
    /// `id` строки `agent_actions` (state=proposed) — адрес решения Approve/Reject.
    pub action_id: i64,
}

/// Событие агент-стрима для клиента (дискриминировано по `type`, camelCase) — СТАБИЛЬНЫЙ JSON-контракт.
/// Зеркалит [`AgentEvent`] ядра 1:1 по вариантам, но это СВОЙ wire-тип (контракт отвязан от внутреннего
/// enum; `non_exhaustive` ядра проявляется обязательным `_`-рукавом в [`map_agent_event`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentStreamEvent {
    /// Дельта контента ассистента (стрим токенов модели).
    AssistantToken { text: String },
    /// Намерение вызвать инструмент ДО исполнения. `id` коррелирует с `toolResult`.
    ToolCall {
        id: String,
        kind: String,
        args: String,
    },
    /// Результат исполнения инструмента. `id` == `id` соответствующего `toolCall`. `isError` —
    /// инструмент вернул ошибку (модель может восстановиться). serde `rename_all` контейнера НЕ
    /// каскадирует в поля struct-вариантов enum — camelCase для составного имени задаём ЯВНО.
    ToolResult {
        id: String,
        content: String,
        #[serde(rename = "isError")]
        is_error: bool,
    },
    /// Загрузка контекстного окна модели (токены): питает %-бар «used/window».
    ContextUsage { used: usize, window: usize },
    /// Changeset, ожидающий решения (Confirm-тир) ЛИБО уведомление перед авто-применением. `runId`
    /// задаём ЯВНО (rename_all не каскадирует в struct-варианты enum).
    Proposal {
        #[serde(rename = "runId")]
        run_id: i64,
        files: Vec<AgentProposedFile>,
    },
    /// Пер-файловый диф changeset'а (эмитится после Proposal, по одному на файл).
    Diff {
        path: String,
        add: u32,
        del: u32,
        status: AgentFileStatus,
    },
    /// Финальный ответ агента (модель завершила ход без новых tool_call).
    Final { text: String },
    /// Терминальная ошибка хода (исчерпан бюджет инициации стрима / провайдер упал и т.п.).
    Error { message: String },
    /// Exec-предложение (Фаза-3 SANDBOX-6c): зеркало [`AgentEvent::ExecProposal`]. `summary` —
    /// редакция-безопасный силуэт (имя инструмента + счётчики), не сырые argv/env. `runId`/`actionId`
    /// заданы ЯВНО (rename_all не каскадирует в struct-варианты enum).
    ExecProposal {
        #[serde(rename = "runId")]
        run_id: i64,
        #[serde(rename = "actionId")]
        action_id: i64,
        summary: String,
    },
    /// Результат исполненного exec (зеркало [`AgentEvent::ExecResult`]): exit-код + finalized, БЕЗ сырого
    /// stdout/stderr (приватность). `runId`/`actionId`/`exitCode` — явный camelCase.
    ExecResult {
        #[serde(rename = "runId")]
        run_id: i64,
        #[serde(rename = "actionId")]
        action_id: i64,
        #[serde(rename = "exitCode")]
        exit_code: i32,
        finalized: bool,
    },
}

/// Маппер `&AgentEvent` → [`AgentStreamEvent`] (контракт «бэкенд → клиент»). Возвращает `Option`: будущее
/// событие ядра, для которого СОЗНАТЕЛЬНО нет представления на проводе, маппится в `None` (его молча НЕ
/// стримим). Матч ЭКСЗАСТИВНЫЙ намеренно: `AgentEvent` `#[non_exhaustive]` снаружи крейта, но wire.rs
/// живёт В `nexus-core`, поэтому новый вариант ядра ВЫЗОВЕТ ОШИБКУ КОМПИЛЯЦИИ здесь — и заставит явно
/// решить его wire-маппинг (`Some(...)` или осознанный `None`), а не уронит его молча. Это и есть гарантия
/// «контракт desktop↔agentd не разъедется при росте ядра».
pub fn map_agent_event(ev: &AgentEvent) -> Option<AgentStreamEvent> {
    Some(match ev {
        AgentEvent::AssistantToken(text) => AgentStreamEvent::AssistantToken { text: text.clone() },
        AgentEvent::ToolCall { id, kind, args } => AgentStreamEvent::ToolCall {
            id: id.clone(),
            kind: kind.clone(),
            args: args.clone(),
        },
        AgentEvent::ToolResult {
            id,
            content,
            is_error,
        } => AgentStreamEvent::ToolResult {
            id: id.clone(),
            content: content.clone(),
            is_error: *is_error,
        },
        AgentEvent::ContextUsage { used, window } => AgentStreamEvent::ContextUsage {
            used: *used,
            window: *window,
        },
        AgentEvent::Proposal { run_id, files } => AgentStreamEvent::Proposal {
            run_id: *run_id,
            files: files
                .iter()
                .map(|f| AgentProposedFile {
                    path: f.path.clone(),
                    add: f.add,
                    del: f.del,
                    status: f.status.into(),
                    action_id: f.action_id,
                })
                .collect(),
        },
        AgentEvent::Diff {
            path,
            add,
            del,
            status,
        } => AgentStreamEvent::Diff {
            path: path.clone(),
            add: *add,
            del: *del,
            status: (*status).into(),
        },
        AgentEvent::Final(text) => AgentStreamEvent::Final { text: text.clone() },
        AgentEvent::Error(message) => AgentStreamEvent::Error {
            message: message.clone(),
        },
        AgentEvent::ExecProposal {
            run_id,
            action_id,
            summary,
        } => AgentStreamEvent::ExecProposal {
            run_id: *run_id,
            action_id: *action_id,
            summary: summary.clone(),
        },
        AgentEvent::ExecResult {
            run_id,
            action_id,
            exit_code,
            finalized,
        } => AgentStreamEvent::ExecResult {
            run_id: *run_id,
            action_id: *action_id,
            exit_code: *exit_code,
            finalized: *finalized,
        },
        // Матч НАМЕРЕННО экзаустивный (без `_`): wire.rs в `nexus-core` видит все варианты `AgentEvent`,
        // и новый вариант ядра ДОЛЖЕН уронить компиляцию здесь — чтобы его wire-маппинг решили явно
        // (`Some(...)` либо осознанный `None`), а не уронили молча. Это и держит контракт desktop↔agentd.
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::event::ProposedFile;
    use serde_json::json;

    fn to_json(ev: &AgentEvent) -> serde_json::Value {
        serde_json::to_value(map_agent_event(ev).unwrap()).unwrap()
    }

    #[test]
    fn maps_newtype_variants_that_core_cannot_serialize() {
        // Эти ядро НЕ сериализует напрямую (newtype + internal-tag) — DTO решает.
        assert_eq!(
            to_json(&AgentEvent::AssistantToken("hi".into())),
            json!({"type":"assistantToken","text":"hi"})
        );
        assert_eq!(
            to_json(&AgentEvent::Final("done".into())),
            json!({"type":"final","text":"done"})
        );
        assert_eq!(
            to_json(&AgentEvent::Error("boom".into())),
            json!({"type":"error","message":"boom"})
        );
    }

    #[test]
    fn maps_struct_variants_with_camelcase() {
        assert_eq!(
            to_json(&AgentEvent::ToolCall {
                id: "c1".into(),
                kind: "note.create".into(),
                args: "{}".into()
            }),
            json!({"type":"toolCall","id":"c1","kind":"note.create","args":"{}"})
        );
        assert_eq!(
            to_json(&AgentEvent::ToolResult {
                id: "c1".into(),
                content: "ok".into(),
                is_error: true
            }),
            json!({"type":"toolResult","id":"c1","content":"ok","isError":true})
        );
        assert_eq!(
            to_json(&AgentEvent::ContextUsage {
                used: 10,
                window: 100
            }),
            json!({"type":"contextUsage","used":10,"window":100})
        );
    }

    #[test]
    fn maps_proposal_and_diff_with_explicit_run_id() {
        let ev = AgentEvent::Proposal {
            run_id: 42,
            files: vec![ProposedFile {
                path: "Notes/a.md".into(),
                add: 3,
                del: 1,
                status: FileStatus::Edit,
                action_id: 7,
            }],
        };
        assert_eq!(
            to_json(&ev),
            json!({"type":"proposal","runId":42,"files":[
                {"path":"Notes/a.md","add":3,"del":1,"status":"edit","actionId":7}
            ]})
        );
        assert_eq!(
            to_json(&AgentEvent::Diff {
                path: "Notes/a.md".into(),
                add: 3,
                del: 1,
                status: FileStatus::New
            }),
            json!({"type":"diff","path":"Notes/a.md","add":3,"del":1,"status":"new"})
        );
    }

    #[test]
    fn wire_event_roundtrips() {
        let w = AgentStreamEvent::ToolResult {
            id: "c1".into(),
            content: "ok".into(),
            is_error: false,
        };
        let s = serde_json::to_string(&w).unwrap();
        assert_eq!(serde_json::from_str::<AgentStreamEvent>(&s).unwrap(), w);
    }

    /// 6c-2g: ExecProposal — STRUCT-вариант (не newtype) → camelCase {type, runId, actionId, summary};
    /// round-trip через AgentStreamEvent сохраняет всё (регресс-гард потери newtype-вариантов serde).
    #[test]
    fn exec_proposal_is_struct_variant_roundtrip() {
        let ev = AgentEvent::ExecProposal {
            run_id: 5,
            action_id: 9,
            summary: "shell.run · argv: 2 токен(ов)".into(),
        };
        let v = to_json(&ev);
        assert_eq!(v["type"], "execProposal");
        assert_eq!(v["runId"], 5);
        assert_eq!(v["actionId"], 9);
        assert_eq!(v["summary"], "shell.run · argv: 2 токен(ов)");
        let wire = map_agent_event(&ev).unwrap();
        let s = serde_json::to_string(&wire).unwrap();
        assert_eq!(serde_json::from_str::<AgentStreamEvent>(&s).unwrap(), wire);
    }

    /// 6c-2g: ExecResult — STRUCT-вариант, camelCase {type, runId, actionId, exitCode, finalized};
    /// СОДЕРЖИМОЕ-СВОБОДЕН (нет stdout-поля). Round-trip сохраняет.
    #[test]
    fn exec_result_is_struct_variant_roundtrip() {
        let ev = AgentEvent::ExecResult {
            run_id: 5,
            action_id: 9,
            exit_code: 1,
            finalized: true,
        };
        let v = to_json(&ev);
        assert_eq!(v["type"], "execResult");
        assert_eq!(v["runId"], 5);
        assert_eq!(v["actionId"], 9);
        assert_eq!(v["exitCode"], 1);
        assert_eq!(v["finalized"], true);
        let wire = map_agent_event(&ev).unwrap();
        let s = serde_json::to_string(&wire).unwrap();
        assert_eq!(serde_json::from_str::<AgentStreamEvent>(&s).unwrap(), wire);
    }

    /// 6c-2g: map_agent_event МАПИТ exec-варианты в `Some(...)` (не молчаливый `None`) — экзаустивный
    /// матч компилит-форсит wire-решение для каждого нового core-варианта.
    #[test]
    fn map_agent_event_covers_exec_variants() {
        assert!(map_agent_event(&AgentEvent::ExecProposal {
            run_id: 1,
            action_id: 1,
            summary: "x".into(),
        })
        .is_some());
        assert!(map_agent_event(&AgentEvent::ExecResult {
            run_id: 1,
            action_id: 1,
            exit_code: 0,
            finalized: true,
        })
        .is_some());
    }
}
