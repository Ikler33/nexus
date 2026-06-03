//! Capability-broker, host-сторона (**ADR-002**, §7.4/§7.9). Брокер — РЕАЛЬНАЯ граница прав: на
//! каждый вызов плагина он определяет identity по **capability-токену сессии**, проверяет
//! scoped-права ([`Permissions::check`], Ф2-1) и пишет в **неотключаемый audit-log**. Сам dispatch
//! (реальный I/O к vault/ai) — за [`HostDispatch`].
//!
//! **Identity = токен (а не `pluginId` из payload).** На фронте каждому плагину выдан один
//! `MessagePort` (§7.5), и хост-релей привязывает к нему правильный токен; через IPC (фронт↔Rust)
//! сессию идентифицирует именно токен — он случаен/неугадываем (`getrandom`), проверяется на каждый
//! вызов и мгновенно инвалидируется ревокацией (§7.9). Это закрывает confused-deputy/laundering:
//! плагин A не может предъявить токен B.

use std::collections::HashMap;
use std::path::PathBuf;

use super::permission::{ApiRequest, Denied, Permissions};

/// Capability-токен сессии: 32 случайных байта в hex (неугадываем, §7.9). Источник identity на IPC.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CapToken(String);

impl CapToken {
    /// Генерирует криптослучайный токен. Паника при недоступности системного RNG — невосстановимо
    /// и крайне маловероятно на десктопе (лучше упасть, чем выдать слабый токен).
    fn generate() -> Self {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).expect("системный RNG недоступен");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(64);
        for b in bytes {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        }
        CapToken(s)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Сессия плагина: его права + корень vault (для резолва путей при dispatch).
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
    /// Токен не привязан к сессии (неизвестный/отозванный плагин) — fail-closed.
    UnknownSession,
    /// Право не выдано / путь вне scope / хост не в allowlist и т.п. (см. [`Denied`]).
    Denied(Denied),
}

impl std::fmt::Display for BrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrokerError::UnknownSession => write!(f, "сессия не найдена (токен невалиден/отозван)"),
            BrokerError::Denied(d) => write!(f, "{d}"),
        }
    }
}

/// Реальный исполнитель авторизованного вызова (vault/ai I/O). Брокер сам I/O не делает —
/// он авторизует и аудитит, а dispatch уводит в этот слой (через `vault::resolve_vault_path` + db/ai).
pub trait HostDispatch {
    fn dispatch(&mut self, session: &PluginSession, req: &ApiRequest) -> Result<String, String>;
}

/// Host-сторона capability-брокера: токен → сессия (identity) + неотключаемый audit.
#[derive(Debug, Default)]
pub struct PluginBroker {
    sessions: HashMap<CapToken, PluginSession>,
    audit: AuditLog,
}

impl PluginBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Открывает сессию плагину, выдавая новый capability-токен (хост зовёт при загрузке плагина).
    pub fn open_session(&mut self, session: PluginSession) -> CapToken {
        let token = CapToken::generate();
        self.sessions.insert(token.clone(), session);
        token
    }

    /// Отзывает сессию (disable/uninstall/смена прав) — токен немедленно невалиден (§7.9 ревокация).
    pub fn revoke(&mut self, token: &CapToken) {
        self.sessions.remove(token);
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn session(&self, token: &CapToken) -> Option<&PluginSession> {
        self.sessions.get(token)
    }

    /// Авторизует вызов: identity по токену → проверка scoped-прав → запись в audit (и успех, и
    /// отказ, §7.9). Identity — из сессии токена, НЕ из запроса → закрывает confused-deputy.
    pub fn authorize(&mut self, token: &CapToken, req: &ApiRequest) -> Result<(), BrokerError> {
        let (id, decision) = match self.sessions.get(token) {
            None => {
                self.audit.record(
                    "<unknown>",
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
        token: &CapToken,
        req: &ApiRequest,
        host: &mut dyn HostDispatch,
    ) -> Result<String, BrokerError> {
        self.authorize(token, req)?;
        let session = self.session(token).ok_or(BrokerError::UnknownSession)?;
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
    fn tokens_are_unique_and_unguessable_length() {
        let mut b = PluginBroker::new();
        let t1 = b.open_session(session("a", r#"{}"#));
        let t2 = b.open_session(session("b", r#"{}"#));
        assert_ne!(t1, t2, "каждая сессия — свой токен");
        assert_eq!(t1.as_str().len(), 64, "32 байта в hex");
        assert!(t1.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn unknown_token_is_denied_and_audited() {
        let mut b = PluginBroker::new();
        let bogus = b.open_session(session("x", r#"{}"#));
        b.revoke(&bogus); // токен больше не валиден
        let r = b.authorize(&bogus, &read("Notes/a.md"));
        assert_eq!(r, Err(BrokerError::UnknownSession));
        assert!(!b.audit().entries().last().unwrap().allowed);
    }

    #[test]
    fn authorizes_within_scope_and_audits_allow() {
        let mut b = PluginBroker::new();
        let t = b.open_session(session("p.a", r#"{"vault:read":["Notes/**"]}"#));
        assert!(b.authorize(&t, &read("Notes/a.md")).is_ok());
        let e = &b.audit().entries()[0];
        assert!(e.allowed && e.plugin_id == "p.a" && e.method == "vault.readFile");
        assert_eq!(e.target.as_deref(), Some("Notes/a.md"));
    }

    #[test]
    fn out_of_scope_denied_and_audited() {
        let mut b = PluginBroker::new();
        let t = b.open_session(session("p.a", r#"{"vault:read":["Notes/**"]}"#));
        let r = b.authorize(&t, &read("Secrets/x.md"));
        assert!(matches!(r, Err(BrokerError::Denied(Denied::OutOfScope(_)))));
        assert!(b.audit().entries()[0].denied_reason.is_some());
    }

    #[test]
    fn identity_is_per_token_confused_deputy() {
        // Узкий плагин и широкий — разные токены. Токен узкого не даёт доступ к scope широкого.
        let mut b = PluginBroker::new();
        let narrow = b.open_session(session("narrow", r#"{"vault:read":["Notes/**"]}"#));
        let wide = b.open_session(session("wide", r#"{"vault:read":["**"]}"#));
        assert!(b.authorize(&narrow, &read("Secrets/x.md")).is_err());
        assert!(b.authorize(&wide, &read("Secrets/x.md")).is_ok());
    }

    #[test]
    fn revoked_token_is_denied() {
        let mut b = PluginBroker::new();
        let t = b.open_session(session("p", r#"{"vault:read":["**"]}"#));
        assert!(b.authorize(&t, &read("a.md")).is_ok());
        b.revoke(&t);
        assert_eq!(
            b.authorize(&t, &read("a.md")),
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
        let t = b.open_session(session("p", r#"{"vault:read":["Notes/**"]}"#));
        let mut host = EchoHost;
        assert_eq!(
            b.handle(&t, &read("Notes/a.md"), &mut host).unwrap(),
            "p:vault.readFile:Notes/a.md"
        );
        assert!(b.handle(&t, &read("Other/a.md"), &mut host).is_err());
    }
}
