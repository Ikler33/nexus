//! ACP — Agent Client Protocol (хэндролл поверх нашего JSON-RPC framing). Две стороны:
//!
//! - **ACP-1 — КЛИЕНТ** ([`client`]): драйвит ВНЕШНИЙ ACP-агент (Hermes и пр.), спавненный подпроцессом
//!   через [`super::StdioTransport`]. Спека/решение (крейт vs хэндролл) — `docs/specs/acp-client.md`.
//!   Маппинг ACP↔наш `AgentStreamEvent` — на стороне desktop `AcpBackend`.
//! - **ACP-2 — СЕРВЕР** ([`server`]): ИНВЕРСИЯ клиента — мы хостим ACP-агента (Castor) поверх stdio,
//!   а внешний ACP-клиент (Zed/JetBrains или наш `AcpClient`) драйвит наш прогон. Подкоманда `nexus acp`.
//!   Спека — `docs/specs/acp-server.md`. Default-OFF актуатор / fail-closed permission. Outbound-провод
//!   (session/update, request_permission, ответы) эмитится через `serde_json::json!` — НЕ через Serialize
//!   (юнионы с `#[serde(other)]` несериализуемы); входящие client-запросы парсятся в `schema`-типы.

pub mod client;
pub mod schema;
pub mod server;

pub use client::{AcpClient, InboundPermission};
pub use server::{serve_acp, AcpServerConfig};

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
