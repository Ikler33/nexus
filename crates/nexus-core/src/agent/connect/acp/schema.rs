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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    pub cwd: PathBuf,
    /// MCP-серверы для сессии (ACP-1: пусто).
    pub mcp_servers: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResult {
    pub session_id: String,
}

// ── session/prompt ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelParams {
    pub session_id: String,
}

// ── session/update (notification от агента) ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    pub session_id: String,
    #[serde(flatten)]
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
    /// user_message_chunk / plan / available_commands_update / current_mode_update и пр. — десериализуем,
    /// но в первом срезе игнорируем (отложено).
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
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hi"}
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
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "title": "edit Notes/A.md",
            "kind": "edit",
            "status": "pending",
            "content": [{"type": "diff", "path": "Notes/A.md", "oldText": null, "newText": "x\ny"}]
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

        // unknown sessionUpdate variant → Other (forward-compat, не ошибка)
        let n3: SessionNotification = serde_json::from_value(serde_json::json!({
            "sessionId": "s1", "sessionUpdate": "current_mode_update", "currentModeId": "x"
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
    }
}
