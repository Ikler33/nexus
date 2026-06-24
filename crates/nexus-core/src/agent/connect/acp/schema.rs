//! ACP-1 — wire-типы ACP (Agent Client Protocol) v1 STABLE, которые нужны клиенту для драйва внешнего
//! агента. Хэндролл (см. ADR в `docs/specs/acp-client.md`): крейт `agent-client-protocol` НЕ берём
//! (tokio-несовместимый рантайм / тяжёлые deps); ACP — JSON-RPC 2.0 line-delimited, идентично нашему
//! framing'у, поэтому переиспользуем `RpcMessage`/`Transport` + сериализуем эти структуры в `params`.
//!
//! ⚠️ serde: `rename_all="camelCase"` НЕ каскадирует в поля struct-вариантов enum (см. `wire.rs:126`) —
//! где нужно, задаём имена явно. `#[serde(other)]` на юнионах = forward-compat: незнакомый вариант →
//! `Other`, не ошибка парса (ACP помечен unstable-эволюционирующим — мы целим v1-stable-подмножество).
//!
//! Контракт-тест (`acp_schema_roundtrips_real_payloads`) пинит реальные ACP-payload'ы → дрейф схемы падает
//! громко (`feedback_mock_must_match_backend`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── initialize ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// ВНИМАНИЕ: ACP `protocolVersion` — ЦЕЛОЕ (напр. `1`), НЕ строка (в отличие от нашего AGENT-CONNECT "1.0").
    pub protocol_version: u16,
    pub client_capabilities: ClientCapabilities,
}

/// Возможности клиента. ACP-1: ВСЁ false — агент правит СВОЙ fs/terminal сам (наш «актуатор» = только
/// решение по `request_permission`); fs/terminal-callbacks к клиенту в первом срезе НЕ принимаем.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    pub fs: FsCaps,
    pub terminal: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCaps {
    pub read_text_file: bool,
    pub write_text_file: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u16,
    // agentCapabilities / authMethods / agentInfo игнорируем в первом срезе.
}

// ── session/new ─────────────────────────────────────────────────────────────────────────────────

/// ACP-2: ДВУСТОРОННИЙ — ACP-1-клиент СЕРИАЛИЗУЕТ при исходящем `session/new`, ACP-2-сервер
/// (`super::server`) ДЕСЕРИАЛИЗУЕТ при входящем запросе. `cwd` сервером ЛОГИРУЕТСЯ, но НЕ репойнтит vault
/// (vault фиксирован `--vault`; R7); `mcp_servers` парсятся-и-игнорируются (логируются, если непусты).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    #[serde(default)]
    pub cwd: PathBuf,
    /// MCP-серверы для сессии (ACP-1: пусто; ACP-2: парсятся-и-игнорируются).
    #[serde(default)]
    pub mcp_servers: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResult {
    pub session_id: String,
}

// ── session/prompt ──────────────────────────────────────────────────────────────────────────────

/// ACP-2: ДВУСТОРОННИЙ (см. [`NewSessionParams`]). Сервер собирает текст хода из `Text`-блоков `prompt`
/// (Other/image/audio игнорируются).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptParams {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    pub stop_reason: StopReason,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// image/audio/resource_link/resource — приходить МОГУТ; в первом срезе игнорируем содержимое.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

// ── session/cancel (notification) ─────────────────────────────────────────────────────────────────

/// ACP-2: ДВУСТОРОННИЙ (см. [`NewSessionParams`]). `session/cancel` приходит как notification (а у
/// некоторых клиентов — как request); сервер взводит кооперативный cancel-флаг сессии.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelParams {
    pub session_id: String,
}

// ── session/update (notification от агента) ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    pub session_id: String,
    /// ACP-спека: `update` — ВЛОЖЕННЫЙ объект (не flatten!). Реальный агент (Hermes 0.17) шлёт
    /// `{"sessionId":…,"update":{"sessionUpdate":…,…}}`. Раньше тут стоял `#[serde(flatten)]`
    /// (ждал плоско `{"sessionId":…,"sessionUpdate":…}`) — наш мок повторял ту же НЕВЕРНУЮ форму,
    /// e2e был зелёный, а живой Hermes молча не парсился (все стрим-апдейты терялись). Пиннируется
    /// тестом на реальных байтах Hermes (`acp_session_update_matches_real_hermes`).
    pub update: SessionUpdate,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum SessionUpdate {
    AgentMessageChunk {
        content: ContentBlock,
    },
    AgentThoughtChunk {
        content: ContentBlock,
    },
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallUpdate),
    /// ACP-1b: план (todo-список). ACP шлёт ПОЛНЫЙ список записей каждым апдейтом (нет инкрементального
    /// статуса) → маппим в `PlanProposed` (id синтезируем по индексу — позиционно стабилен в ходе).
    Plan {
        #[serde(default)]
        entries: Vec<PlanEntry>,
    },
    /// user_message_chunk / available_commands_update / current_mode_update и пр. — десериализуем,
    /// но игнорируем (отложено).
    #[serde(other)]
    Other,
}

/// ACP-1b: запись плана. `priority` парсится (forward-compat), но в маппинг не идёт (наш `PlanStep` его
/// не несёт). `status` → `AgentPlanStepState`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub content: String,
    #[serde(default)]
    pub priority: AcpPlanPriority,
    #[serde(default)]
    pub status: AcpPlanStatus,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AcpPlanPriority {
    High,
    #[default]
    Medium,
    Low,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AcpPlanStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub tool_call_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub kind: ToolKind,
    #[serde(default)]
    pub status: ToolCallStatus,
    #[serde(default)]
    pub content: Vec<ToolCallContent>,
    #[serde(default)]
    pub raw_input: Option<serde_json::Value>,
}

/// Частичный патч tool_call — все поля Option.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallUpdate {
    pub tool_call_id: String,
    #[serde(default)]
    pub status: Option<ToolCallStatus>,
    #[serde(default)]
    pub content: Option<Vec<ToolCallContent>>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub kind: Option<ToolKind>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    Content {
        content: ContentBlock,
    },
    Diff(Diff),
    /// terminal — отложено.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub path: PathBuf,
    /// `None` → новый файл; иначе правка существующего.
    #[serde(default)]
    pub old_text: Option<String>,
    pub new_text: String,
}

// ── session/request_permission (запрос ОТ агента к клиенту) ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionParams {
    #[serde(default)]
    pub session_id: String,
    pub tool_call: ToolCallUpdate,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    #[serde(default)]
    pub name: String,
    pub kind: PermissionOptionKind,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    /// неизвестный вид опции → трактуем как небезопасный (не выбираем для allow).
    #[serde(other)]
    Other,
}

/// Наш ОТВЕТ на `request_permission`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionResponse {
    pub outcome: PermissionOutcome,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PermissionOutcome {
    Selected {
        #[serde(rename = "optionId")]
        option_id: String,
    },
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_schema_roundtrips_real_payloads() {
        // Пиннированные реальные ACP-payload'ы — дрейф схемы упадёт громко.
        // initialize params
        let p: InitializeParams = serde_json::from_value(serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}
        }))
        .unwrap();
        assert_eq!(p.protocol_version, 1);
        assert!(!p.client_capabilities.terminal);

        // session/update: agent_message_chunk
        let n: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}}
        }))
        .unwrap();
        assert_eq!(n.session_id, "s1");
        assert!(matches!(
            n.update,
            SessionUpdate::AgentMessageChunk {
                content: ContentBlock::Text { ref text }
            } if text == "hi"
        ));

        // session/update: tool_call with a diff
        let n2: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": "t1",
                "title": "edit Notes/A.md",
                "kind": "edit",
                "status": "pending",
                "content": [{"type": "diff", "path": "Notes/A.md", "oldText": null, "newText": "x\ny"}]
            }
        }))
        .unwrap();
        match n2.update {
            SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.tool_call_id, "t1");
                assert_eq!(tc.kind, ToolKind::Edit);
                assert!(matches!(tc.content.first(), Some(ToolCallContent::Diff(_))));
            }
            _ => panic!("expected tool_call"),
        }

        // ACP-1b: session/update tool_call с ДВУМЯ diff'ами (мульти-файловый permission).
        let n2b: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": "t2",
                "content": [
                    {"type": "diff", "path": "Notes/A.md", "oldText": null, "newText": "x"},
                    {"type": "diff", "path": "Notes/B.md", "oldText": "p", "newText": "q\nr"}
                ]
            }
        }))
        .unwrap();
        match n2b.update {
            SessionUpdate::ToolCallUpdate(u) => {
                let diffs: Vec<_> = u
                    .content
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|c| match c {
                        ToolCallContent::Diff(d) => Some(d),
                        _ => None,
                    })
                    .collect();
                assert_eq!(diffs.len(), 2);
                assert_eq!(diffs[0].path.to_string_lossy(), "Notes/A.md");
                assert!(diffs[0].old_text.is_none());
                assert_eq!(diffs[1].path.to_string_lossy(), "Notes/B.md");
                assert_eq!(diffs[1].old_text.as_deref(), Some("p"));
            }
            _ => panic!("expected tool_call_update"),
        }

        // ACP-1b: session/update plan (полный список записей со статусами/приоритетами).
        let np: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "plan",
                "entries": [
                    {"content": "research", "priority": "high", "status": "in_progress"},
                    {"content": "write", "priority": "medium", "status": "pending"}
                ]
            }
        }))
        .unwrap();
        match np.update {
            SessionUpdate::Plan { entries } => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].content, "research");
                assert_eq!(entries[0].priority, AcpPlanPriority::High);
                assert_eq!(entries[0].status, AcpPlanStatus::InProgress);
                assert_eq!(entries[1].status, AcpPlanStatus::Pending);
            }
            _ => panic!("expected plan"),
        }

        // ACP-1b: plan с неизвестными priority/status → Other (forward-compat, не ошибка).
        let np2: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "plan",
                "entries": [{"content": "x", "priority": "urgent", "status": "blocked"}]
            }
        }))
        .unwrap();
        match np2.update {
            SessionUpdate::Plan { entries } => {
                assert_eq!(entries[0].priority, AcpPlanPriority::Other);
                assert_eq!(entries[0].status, AcpPlanStatus::Other);
            }
            _ => panic!("expected plan"),
        }

        // unknown sessionUpdate variant → Other (forward-compat, не ошибка)
        let n3: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "s1", "update": {"sessionUpdate": "current_mode_update", "currentModeId": "x"}
        }))
        .unwrap();
        assert!(matches!(n3.update, SessionUpdate::Other));

        // request_permission params + outcome serialize
        let rp: RequestPermissionParams = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "toolCall": {"toolCallId": "t1"},
            "options": [
                {"optionId": "a", "name": "Allow", "kind": "allow_once"},
                {"optionId": "d", "name": "Deny", "kind": "reject_once"}
            ]
        }))
        .unwrap();
        assert_eq!(rp.options.len(), 2);
        assert_eq!(rp.options[0].kind, PermissionOptionKind::AllowOnce);
        let out = serde_json::to_value(RequestPermissionResponse {
            outcome: PermissionOutcome::Selected {
                option_id: "a".into(),
            },
        })
        .unwrap();
        assert_eq!(
            out,
            serde_json::json!({"outcome": {"outcome": "selected", "optionId": "a"}})
        );
        let cancelled = serde_json::to_value(RequestPermissionResponse {
            outcome: PermissionOutcome::Cancelled,
        })
        .unwrap();
        assert_eq!(
            cancelled,
            serde_json::json!({"outcome": {"outcome": "cancelled"}})
        );

        // stop reason
        let pr: PromptResult =
            serde_json::from_value(serde_json::json!({"stopReason": "end_turn"})).unwrap();
        assert_eq!(pr.stop_reason, StopReason::EndTurn);

        // ── ACP-2 (СЕРВЕР): входящие client-запросы ДЕСЕРИАЛИЗУЮТСЯ (инверсия ACP-1) ──
        // session/new params: cwd + mcpServers (оба default-толерантны).
        let ns: NewSessionParams = serde_json::from_value(serde_json::json!({
            "cwd": "/abs/vault",
            "mcpServers": [{"name": "x"}]
        }))
        .unwrap();
        assert_eq!(ns.cwd.to_string_lossy(), "/abs/vault");
        assert_eq!(ns.mcp_servers.len(), 1);
        // отсутствующие cwd/mcpServers → дефолты (IDE-интероп).
        let ns2: NewSessionParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(ns2.cwd.as_os_str().len(), 0);
        assert!(ns2.mcp_servers.is_empty());

        // session/prompt params: sessionId + prompt[] (Text-блоки).
        let pp: PromptParams = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "prompt": [{"type": "text", "text": "do it"}, {"type": "image", "data": "…"}]
        }))
        .unwrap();
        assert_eq!(pp.session_id, "s1");
        assert_eq!(pp.prompt.len(), 2);
        assert!(matches!(
            pp.prompt.first(),
            Some(ContentBlock::Text { text }) if text == "do it"
        ));
        assert!(matches!(pp.prompt.get(1), Some(ContentBlock::Other)));

        // session/cancel params: sessionId.
        let cp: CancelParams =
            serde_json::from_value(serde_json::json!({"sessionId": "s1"})).unwrap();
        assert_eq!(cp.session_id, "s1");
    }

    /// Регрессия: ПИННИРОВАННЫЕ СЫРЫЕ payload'ы реального агента Hermes 0.17 (DeepSeek), снятые
    /// живым ACP-прогоном против `docker exec -i hermes hermes acp` на .28 (2026-06-24). Ловит баг
    /// `#[serde(flatten)]` на `SessionNotification` (раньше тест пиннил ПЛОСКУЮ форму нашего же мока,
    /// а Hermes шлёт ВЛОЖЕННУЮ `{"sessionId":…,"update":{…}}`). НЕ редактировать payload'ы под код —
    /// это байты с провода; код обязан их парсить.
    #[test]
    fn acp_session_update_matches_real_hermes() {
        // initialize result: protocolVersion присутствует (+ agentCapabilities/agentInfo/authMethods).
        let init: InitializeResult = serde_json::from_value(serde_json::json!({
            "agentCapabilities": {"loadSession": true, "promptCapabilities": {"image": true},
                "sessionCapabilities": {"fork": {}, "list": {}, "resume": {}}},
            "agentInfo": {"name": "hermes-agent", "version": "0.17.0"},
            "authMethods": [{"description": "…", "id": "deepseek", "name": "deepseek runtime credentials"}],
            "protocolVersion": 1
        }))
        .unwrap();
        assert_eq!(init.protocol_version, 1);

        // session/new result: sessionId присутствует (+ _meta/models/modes — игнорируем).
        let nr: NewSessionResult = serde_json::from_value(serde_json::json!({
            "_meta": {"hermes": {"sessionProvenance": {"sessionKind": "root"}}},
            "models": {"currentModelId": "deepseek:deepseek-v4-flash"},
            "modes": {"currentModeId": "default"},
            "sessionId": "39226b5d-5d12-4a6f-a22e-3701ad92f8d3"
        }))
        .unwrap();
        assert_eq!(nr.session_id, "39226b5d-5d12-4a6f-a22e-3701ad92f8d3");

        // session/update agent_message_chunk (ВЛОЖЕННЫЙ update — реальная форма Hermes).
        let amc: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "39226b5d-5d12-4a6f-a22e-3701ad92f8d3",
            "update": {"content": {"text": "\n\nГ", "type": "text"}, "sessionUpdate": "agent_message_chunk"}
        }))
        .unwrap();
        assert!(matches!(
            amc.update,
            SessionUpdate::AgentMessageChunk { content: ContentBlock::Text { ref text } } if text == "\n\nГ"
        ));

        // session/update agent_thought_chunk → Other (мы его не используем, но НЕ должны падать).
        let atc: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "s", "update": {"content": {"text": "The", "type": "text"}, "sessionUpdate": "agent_thought_chunk"}
        }))
        .unwrap();
        assert!(matches!(
            atc.update,
            SessionUpdate::AgentThoughtChunk { .. }
        ));

        // session/update tool_call с реальными полями Hermes (locations/rawInput игнорируются).
        let tc: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "39226b5d-5d12-4a6f-a22e-3701ad92f8d3",
            "update": {
                "content": [{"content": {"text": "Preparing write to /opt/hermes/hello.md.", "type": "text"}, "type": "content"}],
                "kind": "edit",
                "locations": [{"path": "/opt/hermes/hello.md"}],
                "title": "write: /opt/hermes/hello.md",
                "toolCallId": "tc-09ab46b1a720",
                "sessionUpdate": "tool_call"
            }
        }))
        .unwrap();
        match tc.update {
            SessionUpdate::ToolCall(t) => {
                assert_eq!(t.tool_call_id, "tc-09ab46b1a720");
                assert_eq!(t.kind, ToolKind::Edit);
            }
            _ => panic!("expected tool_call"),
        }

        // session/request_permission params реального Hermes (toolCall.content[].diff + options).
        let rp: RequestPermissionParams = serde_json::from_value(serde_json::json!({
            "options": [
                {"kind": "allow_once", "name": "Allow edit", "optionId": "allow_once"},
                {"kind": "reject_once", "name": "Deny", "optionId": "deny"}
            ],
            "sessionId": "39226b5d-5d12-4a6f-a22e-3701ad92f8d3",
            "toolCall": {
                "content": [{"newText": "Hello from Hermes.", "path": "/opt/hermes/hello.md", "type": "diff"}],
                "kind": "edit",
                "rawInput": {"tool": "write_file", "arguments": {"path": "/opt/hermes/hello.md", "content": "Hello from Hermes."}},
                "status": "pending",
                "title": "Approve edit: /opt/hermes/hello.md",
                "toolCallId": "edit-approval-1"
            }
        }))
        .unwrap();
        assert_eq!(rp.options[0].kind, PermissionOptionKind::AllowOnce);
        let diffs: Vec<_> = rp
            .tool_call
            .content
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| match c {
                ToolCallContent::Diff(d) => Some(d),
                _ => None,
            })
            .collect();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path.to_string_lossy(), "/opt/hermes/hello.md");
        assert_eq!(diffs[0].new_text, "Hello from Hermes.");

        // prompt result реального Hermes: stopReason + usage (usage игнорируем).
        let pr: PromptResult = serde_json::from_value(serde_json::json!({
            "stopReason": "end_turn",
            "usage": {"inputTokens": 50118, "outputTokens": 526, "totalTokens": 50644}
        }))
        .unwrap();
        assert_eq!(pr.stop_reason, StopReason::EndTurn);
    }
}
