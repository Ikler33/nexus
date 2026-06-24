//! ACP-1 — клиент Agent Client Protocol (хэндролл поверх нашего JSON-RPC framing). Драйвит ВНЕШНИЙ
//! ACP-агент (Hermes и пр.), спавненный подпроцессом через [`super::StdioTransport`]. Спека/решение
//! (крейт vs хэндролл) — `docs/specs/acp-client.md`. Маппинг ACP↔наш `AgentStreamEvent` — на стороне
//! desktop `AcpBackend` (нужен `tauri::Channel`/`AppState`).

pub mod client;
pub mod schema;

pub use client::{AcpClient, InboundPermission};

/// Версия ACP-протокола, которую объявляем в `initialize` (v1 STABLE; целое, не строка).
pub const ACP_PROTOCOL_VERSION: u16 = 1;

/// ACP `ToolKind` → отображаемая строка для нашего `AgentStreamEvent::ToolCall.kind` (обратное к
/// `super::super::acp_tool_kind`). Незнакомый/`Other` → `"other"`.
pub fn acp_kind_to_display(kind: schema::ToolKind) -> &'static str {
    use schema::ToolKind::*;
    match kind {
        Read => "read",
        Edit => "edit",
        Delete => "delete",
        Move => "move",
        Search => "search",
        Execute => "execute",
        Think => "think",
        Fetch => "fetch",
        SwitchMode => "switch_mode",
        Other => "other",
    }
}
