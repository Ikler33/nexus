//! Capability-broker, host-сторона (**ADR-002**, §7.4). Брокер — РЕАЛЬНАЯ граница прав: на каждый
//! вызов плагина он определяет identity **по порту** (не по `pluginId` из payload — иначе confused
//! deputy: плагин A назвался бы B и забрал его права), проверяет scoped-права ([`Permissions::check`],
//! Ф2-1) и пишет в **неотключаемый audit-log** (§7.9). Сам dispatch (реальный I/O к vault/ai) —
//! отдельным слоем через [`HostDispatch`]; транспорт (MessagePort/iframe) и capability-токены — Ф2-2b.
//!
//! Здесь — чистая, тестируемая модель: сессии, авторизация, audit, ревокация. Никакого ввода-вывода.

use std::collections::HashMap;
use std::path::PathBuf;

use super::permission::{ApiRequest, Denied, Permissions};

/// Идентификатор выделенного плагину порта/канала — источник истины identity (§7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortId(pub u64);

/// Сессия плагина: привязана к порту, несёт его права и корень vault (для резолва путей при dispatch).
#[derive(Debug, Clone)]
pub struct PluginSession {
    pub id: String,
    pub permissions: Permissions,
    pub vault_root: PathBuf,
}

/// Запись audit-лога (неотключаемого): кто, что, по какой цели и с каким решением.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub plugin_id: String,
    pub method: String,
    pub target: Option<String>,
    pub allowed: bool,
    pub denied_reason: Option<String>,
}

/// Неотключаемый журнал доступа (на брокер). Только добавление; чистить нельзя by design.
#[derive(Debug, Default)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
}

impl AuditLog {
    fn record(&mut self, plugin_id: &str, req: &ApiRequest, decision: &Result<(), Denied>) {
        self.entries.push(AuditEntry {
            plugin_id: plugin_id.to_string(),
            method: req.method.to_string(),
            target: req.path.or(req.host).map(|s| s.to_string()),
            allowed: decision.is_ok(),
            denied_reason: decision.as_ref().err().map(|d| d.to_string()),
        });
    }
    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Ошибка авторизации брокера.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerError {
    /// Порт не привязан к сессии (неизвестный/отозванный плагин) — fail-closed.
    UnknownSession,
    /// Право не выдано / путь вне scope / хост не в allowlist и т.п. (см. [`Denied`]).
    Denied(Denied),
}

impl std::fmt::Display for BrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrokerError::UnknownSession => write!(f, "сессия не найдена (порт не зарегистрирован)"),
            BrokerError::Denied(d) => write!(f, "{d}"),
        }
    }
}

/// Реальный исполнитель авторизованного вызова (vault/ai I/O). Брокер сам I/O не делает —
/// он авторизует и аудитит, а dispatch уводит в этот слой (Ф2-2b: vault::resolve_vault_path + db/ai).
pub trait HostDispatch {
    fn dispatch(&mut self, session: &PluginSession, req: &ApiRequest) -> Result<String, String>;
}

/// Host-сторона capability-брокера: порт → сессия (identity) + неотключаемый audit.
#[derive(Debug, Default)]
pub struct PluginBroker {
    sessions: HashMap<PortId, PluginSession>,
    audit: AuditLog,
}

impl PluginBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Привязывает сессию к порту (выдаётся хостом при загрузке плагина).
    pub fn register(&mut self, port: PortId, session: PluginSession) {
        self.sessions.insert(port, session);
    }

    /// Отзывает сессию (disable/uninstall/смена прав) — порт больше не авторизуется (§7.9 ревокация).
    pub fn revoke(&mut self, port: PortId) {
        self.sessions.remove(&port);
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    fn session(&self, port: PortId) -> Option<&PluginSession> {
        self.sessions.get(&port)
    }

    /// Авторизует вызов: identity по порту → проверка scoped-прав → запись в audit. Возвращает `Ok`
    /// (можно диспатчить) либо `Err`. **И отказ, и успех аудитятся** (§7.9). Identity берётся из
    /// сессии порта, а НЕ из запроса — закрывает confused-deputy/capability-laundering.
    pub fn authorize(&mut self, port: PortId, req: &ApiRequest) -> Result<(), BrokerError> {
        // Сначала вычисляем id + решение (заимствование сессии завершается до записи в audit).
        let (id, decision) = match self.sessions.get(&port) {
            None => {
                // Неизвестный порт: тоже фиксируем попытку (id неизвестен).
                self.audit.record(
                    "<unknown-port>",
                    req,
                    &Err(Denied::UnknownMethod(req.method.to_string())),
                );
                return Err(BrokerError::UnknownSession);
            }
            Some(s) => (s.id.clone(), s.permissions.check(req)),
        };
        self.audit.record(&id, req, &decision);
        decision.map_err(BrokerError::Denied)
    }

    /// Полный путь вызова (§7.4): авторизация → (при успехе) dispatch через [`HostDispatch`].
    pub fn handle(
        &mut self,
        port: PortId,
        req: &ApiRequest,
        host: &mut dyn HostDispatch,
    ) -> Result<String, BrokerError> {
        self.authorize(port, req)?;
        let session = self.session(port).ok_or(BrokerError::UnknownSession)?;
        host.dispatch(session, req)
            .map_err(|e| BrokerError::Denied(Denied::UnknownMethod(e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, perms_json: &str) -> PluginSession {
        PluginSession {
            id: id.to_string(),
            permissions: serde_json::from_str(perms_json).unwrap(),
            vault_root: PathBuf::from("/vault"),
        }
    }
    fn read(path: &str) -> ApiRequest<'_> {
        ApiRequest {
            method: "vault.readFile",
            path: Some(path),
            host: None,
        }
    }

    #[test]
    fn unknown_port_is_denied_and_audited() {
        let mut b = PluginBroker::new();
        let r = b.authorize(PortId(99), &read("Notes/a.md"));
        assert_eq!(r, Err(BrokerError::UnknownSession));
        assert_eq!(
            b.audit().len(),
            1,
            "попытка с неизвестного порта тоже в audit"
        );
        assert!(!b.audit().entries()[0].allowed);
    }

    #[test]
    fn authorizes_within_scope_and_audits_allow() {
        let mut b = PluginBroker::new();
        b.register(PortId(1), session("p.a", r#"{"vault:read":["Notes/**"]}"#));
        assert!(b.authorize(PortId(1), &read("Notes/a.md")).is_ok());
        let e = &b.audit().entries()[0];
        assert!(e.allowed && e.plugin_id == "p.a" && e.method == "vault.readFile");
        assert_eq!(e.target.as_deref(), Some("Notes/a.md"));
    }

    #[test]
    fn out_of_scope_denied_and_audited() {
        let mut b = PluginBroker::new();
        b.register(PortId(1), session("p.a", r#"{"vault:read":["Notes/**"]}"#));
        let r = b.authorize(PortId(1), &read("Secrets/x.md"));
        assert!(matches!(r, Err(BrokerError::Denied(Denied::OutOfScope(_)))));
        assert!(!b.audit().entries()[0].allowed);
        assert!(b.audit().entries()[0].denied_reason.is_some());
    }

    #[test]
    fn identity_is_per_port_not_payload_confused_deputy() {
        // Плагин A (узкие права) на порту 1; плагин B (широкие) на порту 2.
        let mut b = PluginBroker::new();
        b.register(
            PortId(1),
            session("narrow", r#"{"vault:read":["Notes/**"]}"#),
        );
        b.register(PortId(2), session("wide", r#"{"vault:read":["**"]}"#));
        // С порта 1 нельзя дотянуться до того, что разрешено только порту 2 — права берутся ПО ПОРТУ.
        assert!(b.authorize(PortId(1), &read("Secrets/x.md")).is_err());
        assert!(b.authorize(PortId(2), &read("Secrets/x.md")).is_ok());
    }

    #[test]
    fn revoked_session_is_denied() {
        let mut b = PluginBroker::new();
        b.register(PortId(1), session("p", r#"{"vault:read":["**"]}"#));
        assert!(b.authorize(PortId(1), &read("a.md")).is_ok());
        b.revoke(PortId(1));
        assert_eq!(
            b.authorize(PortId(1), &read("a.md")),
            Err(BrokerError::UnknownSession)
        );
    }

    struct EchoHost;
    impl HostDispatch for EchoHost {
        fn dispatch(&mut self, s: &PluginSession, req: &ApiRequest) -> Result<String, String> {
            Ok(format!(
                "{}:{}:{}",
                s.id,
                req.method,
                req.path.unwrap_or("")
            ))
        }
    }

    #[test]
    fn handle_dispatches_only_after_authorize() {
        let mut b = PluginBroker::new();
        b.register(PortId(1), session("p", r#"{"vault:read":["Notes/**"]}"#));
        let mut host = EchoHost;
        assert_eq!(
            b.handle(PortId(1), &read("Notes/a.md"), &mut host).unwrap(),
            "p:vault.readFile:Notes/a.md"
        );
        // Вне scope — dispatch НЕ вызывается (отказ на авторизации).
        assert!(b.handle(PortId(1), &read("Other/a.md"), &mut host).is_err());
    }
}
