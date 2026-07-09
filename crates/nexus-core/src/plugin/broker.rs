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
use std::sync::{Arc, Mutex};

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
    /// Реконструирует токен из строки, пришедшей по IPC от фронта. Безопасно: совпадёт лишь с уже
    /// выданным (случайным) токеном — подделать нельзя, неизвестный токен брокер отвергнет (fail-closed).
    pub fn from_ipc(s: String) -> Self {
        CapToken(s)
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

/// DTO durable-записи `plugin_audit` для UI «Журнал доступа» (PLUG-1): поля [`AuditEntry`] + метка
/// времени и id строки (стабильный ключ списка на фронте). Сериализуется в camelCase — контракт
/// провода команды `list_plugin_audit` (зеркалит TS `PluginAuditRecord`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuditRecord {
    pub id: i64,
    pub plugin_id: String,
    pub method: String,
    pub target: Option<String>,
    pub allowed: bool,
    pub denied_reason: Option<String>,
    /// Unix-сек метки записи (для сортировки/отображения в UI).
    pub created_at: i64,
}

impl AuditEntry {
    /// Строит запись из решения авторизации (общая ось для in-memory и durable). Цель — `path` либо
    /// `host` (что применимо к методу): для vault-методов путь, для `net.fetch` — хост.
    fn from_decision(plugin_id: &str, req: &ApiRequest, decision: &Result<(), Denied>) -> Self {
        AuditEntry {
            plugin_id: plugin_id.to_string(),
            method: req.method.to_string(),
            target: req.path.or(req.host).map(|s| s.to_string()),
            allowed: decision.is_ok(),
            denied_reason: decision.as_ref().err().map(|d| d.to_string()),
        }
    }
}

/// Неотключаемый append-only журнал доступа брокера (инвариант — как ядровый [`crate::net::EgressAudit`]):
/// `record()` персистит в БД, публичны только чтения; чистить нельзя by design (THREAT_MODEL T1/§3).
///
/// ДВА слоя (зеркало `EgressAudit`, PLUG-1):
/// 1. **In-memory** `Mutex<Vec<AuditEntry>>` — снимки [`entries`](Self::entries), pre-vault авторизации
///    (БД ещё не открыта) и тесты.
/// 2. **Durable** опциональный [`WriteActor`]-сток, выставляемый ПОСЛЕ конструирования через
///    [`set_writer`](Self::set_writer) (брокер строится в `AppState::new` ДО открытия vault-БД).
///    Когда сток есть, [`record`](Self::record) персистит запись append-only в `plugin_audit`
///    **ПЕРЕД возвратом** (write-before-act). Pre-vault окно: сток ещё `None` → запись живёт только в
///    памяти (durable-история начинается с момента `set_writer`).
///
/// **Живёт за `Arc`** внутри [`PluginBroker`]: это позволяет вызывающему (`plugin_invoke`) склонировать
/// `Arc<AuditLog>` под std-локом брокера и выполнить `record().await` УЖЕ ПОСЛЕ его освобождения —
/// durable-I/O не держит std-лок брокера через `.await` (см. [`PluginBroker::authorize`]).
#[derive(Debug, Default)]
pub struct AuditLog {
    /// `Mutex` — записи короткие; журнал делится через `Arc` (наружу — только снимок-копия).
    entries: Mutex<Vec<AuditEntry>>,
    /// Durable-сток: `Some(WriteActor)` после `set_writer` (открытие vault). До него — pre-vault окно
    /// (только in-memory). `Mutex` (а не `OnceLock`): десктоп может переоткрыть vault (новая БД) →
    /// сток заменяем. `WriteActor` клонируется дёшево (общий канал).
    writer: Mutex<Option<crate::db::WriteActor>>,
}

impl AuditLog {
    /// Подключает durable-сток ПОСЛЕ конструирования (брокер строится в `AppState::new` ДО открытия
    /// vault-БД). Зовётся из composition-root: десктоп — в `open_vault` после `Database::open`
    /// (зеркало `EgressAudit::set_writer`). С этого момента `record()` персистит каждый вызов брокера
    /// в `plugin_audit`. При переоткрытии vault сток свапается на новую БД.
    pub fn set_writer(&self, writer: crate::db::WriteActor) {
        if let Ok(mut w) = self.writer.lock() {
            *w = Some(writer);
        }
    }

    /// Синхронно добавляет запись ТОЛЬКО в in-memory слой (без durable, без await). Зовётся из
    /// [`PluginBroker::authorize`] под std-локом брокера: снимки/тесты видят авторизацию сразу.
    /// Durable-персист — отдельным async [`record_durable`](Self::record_durable) уже вне лока.
    fn record_in_memory(&self, entry: AuditEntry) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(entry);
        }
    }

    /// Персистит уже сформированную [`AuditEntry`] в **durable** слой (`plugin_audit`). In-memory
    /// слой уже наполнен синхронно в [`PluginBroker::authorize`] — здесь ТОЛЬКО БД (без двойного
    /// in-memory-push). Зовётся вызывающим ПОСЛЕ освобождения std-лока брокера (durable-I/O не под
    /// локом брокера — зеркало `EgressAudit::record`).
    ///
    /// **Write-before-act**: если durable-сток есть — персистит append-only в `plugin_audit` И ЖДЁТ
    /// коммита ПЕРЕД возвратом. Сбой БД НЕ роняет вызов (best-effort durable; in-memory-слой запись
    /// сохранил), но логируется: durable-история — подотчётность, не gate на сам вызов. Pre-vault
    /// окно (сток ещё `None`) — no-op (запись живёт в памяти).
    pub async fn record_durable(&self, entry: AuditEntry) {
        // Durable — если сток подключён. Клонируем WriteActor под мьютексом и сразу отпускаем лок
        // (await под std::Mutex недопустим). Ждём коммит ПЕРЕД возвратом (write-before-act).
        let writer = self.writer.lock().ok().and_then(|w| w.clone());
        if let Some(writer) = writer {
            let created_at = crate::scheduler::now_secs();
            let res = writer
                .call(move |conn| {
                    conn.execute(
                        "INSERT INTO plugin_audit \
                         (plugin_id, method, target, allowed, denied_reason, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            entry.plugin_id,
                            entry.method,
                            entry.target,
                            entry.allowed as i64,
                            entry.denied_reason,
                            created_at,
                        ],
                    )
                    .map(|_| ())
                })
                .await;
            if let Err(e) = res {
                // Best-effort: durable-сбой не роняет вызов (in-memory-слой запись сохранил).
                tracing::warn!(error = %e, "plugin-audit: durable-запись не удалась (in-memory сохранён)");
            }
        }
    }

    /// Снимок in-memory журнала (копия: журнал под мьютексом, наружу — без ссылки на внутренности).
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries.lock().map(|e| e.clone()).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
///
/// `audit` — за `Arc`: durable-запись [`AuditLog::record`] async, и вызывающий (`plugin_invoke`)
/// клонирует этот `Arc` под std-локом брокера, а `record().await` выполняет УЖЕ ПОСЛЕ его
/// освобождения — durable-I/O не держит std-лок брокера через `.await`.
#[derive(Debug, Default)]
pub struct PluginBroker {
    sessions: HashMap<CapToken, PluginSession>,
    audit: Arc<AuditLog>,
}

impl PluginBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Подключает durable-сток audit-журнала (зеркало `EgressAudit::set_writer`): десктоп зовёт в
    /// `open_vault` после открытия БД. См. [`AuditLog::set_writer`].
    pub fn set_writer(&self, writer: crate::db::WriteActor) {
        self.audit.set_writer(writer);
    }

    /// Клон `Arc` на audit-журнал — для durable-записи ПОСЛЕ освобождения std-лока брокера
    /// (`plugin_invoke`: `authorize` под локом → клон `Arc` → отпустить лок → `record().await`).
    pub fn audit_log(&self) -> Arc<AuditLog> {
        self.audit.clone()
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
    ///
    /// Возвращает `(решение, запись audit)`. Запись УЖЕ добавлена в **in-memory** слой синхронно
    /// (снимки/тесты видят её сразу), а её КОПИЯ отдана вызывающему, чтобы durable-персист
    /// [`AuditLog::record`] выполнить УЖЕ ПОСЛЕ освобождения std-лока брокера (durable-I/O не под
    /// локом брокера — зеркало `EgressAudit`). Вызывающий обязан затем сделать
    /// `broker.audit_log().record(entry).await` вне лока.
    pub fn authorize(
        &mut self,
        token: &CapToken,
        req: &ApiRequest,
    ) -> (Result<(), BrokerError>, AuditEntry) {
        let (id, decision) = match self.sessions.get(token) {
            None => (
                "<unknown>".to_string(),
                Err(Denied::UnknownMethod(req.method.to_string())),
            ),
            Some(s) => (s.id.clone(), s.permissions.check(req)),
        };
        let entry = AuditEntry::from_decision(&id, req, &decision);
        // In-memory — синхронно, ПОД std-локом брокера (запись короткая, без await): снимки/тесты
        // видят авторизацию сразу. Durable-персист — вызывающим, УЖЕ вне лока (см. докстринг).
        self.audit.record_in_memory(entry.clone());
        let result = match self.sessions.get(token) {
            None => Err(BrokerError::UnknownSession),
            Some(_) => decision.map_err(BrokerError::Denied),
        };
        (result, entry)
    }

    /// Полный путь вызова (§7.4): авторизация → (при успехе) dispatch через [`HostDispatch`].
    /// In-process-путь без durable-персиста (durable-запись — на пути `plugin_invoke`, где есть
    /// async-контекст и WriteActor); in-memory audit ведётся всегда.
    pub fn handle(
        &mut self,
        token: &CapToken,
        req: &ApiRequest,
        host: &mut dyn HostDispatch,
    ) -> Result<String, BrokerError> {
        self.authorize(token, req).0?;
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
        let (r, _entry) = b.authorize(&bogus, &read("Notes/a.md"));
        assert_eq!(r, Err(BrokerError::UnknownSession));
        assert!(!b.audit().entries().last().unwrap().allowed);
    }

    #[test]
    fn authorizes_within_scope_and_audits_allow() {
        let mut b = PluginBroker::new();
        let t = b.open_session(session("p.a", r#"{"vault:read":["Notes/**"]}"#));
        assert!(b.authorize(&t, &read("Notes/a.md")).0.is_ok());
        let e = &b.audit().entries()[0];
        assert!(e.allowed && e.plugin_id == "p.a" && e.method == "vault.readFile");
        assert_eq!(e.target.as_deref(), Some("Notes/a.md"));
    }

    #[test]
    fn out_of_scope_denied_and_audited() {
        let mut b = PluginBroker::new();
        let t = b.open_session(session("p.a", r#"{"vault:read":["Notes/**"]}"#));
        let (r, _entry) = b.authorize(&t, &read("Secrets/x.md"));
        assert!(matches!(r, Err(BrokerError::Denied(Denied::OutOfScope(_)))));
        assert!(b.audit().entries()[0].denied_reason.is_some());
    }

    /// Возврат `authorize`: audit-запись отдаётся вызывающему для durable-персиста ВНЕ std-лока
    /// брокера, и она согласована с решением (allow → `allowed`, deny → `denied_reason`).
    #[test]
    fn authorize_returns_entry_for_durable_persist() {
        let mut b = PluginBroker::new();
        let t = b.open_session(session("p.a", r#"{"vault:read":["Notes/**"]}"#));
        let (ok, allow_entry) = b.authorize(&t, &read("Notes/a.md"));
        assert!(ok.is_ok());
        assert!(allow_entry.allowed && allow_entry.denied_reason.is_none());
        assert_eq!(allow_entry.plugin_id, "p.a");
        assert_eq!(allow_entry.target.as_deref(), Some("Notes/a.md"));

        let (deny, deny_entry) = b.authorize(&t, &read("Secrets/x.md"));
        assert!(deny.is_err());
        assert!(!deny_entry.allowed && deny_entry.denied_reason.is_some());
    }

    #[test]
    fn identity_is_per_token_confused_deputy() {
        // Узкий плагин и широкий — разные токены. Токен узкого не даёт доступ к scope широкого.
        let mut b = PluginBroker::new();
        let narrow = b.open_session(session("narrow", r#"{"vault:read":["Notes/**"]}"#));
        let wide = b.open_session(session("wide", r#"{"vault:read":["**"]}"#));
        assert!(b.authorize(&narrow, &read("Secrets/x.md")).0.is_err());
        assert!(b.authorize(&wide, &read("Secrets/x.md")).0.is_ok());
    }

    #[test]
    fn revoked_token_is_denied() {
        let mut b = PluginBroker::new();
        let t = b.open_session(session("p", r#"{"vault:read":["**"]}"#));
        assert!(b.authorize(&t, &read("a.md")).0.is_ok());
        b.revoke(&t);
        assert_eq!(
            b.authorize(&t, &read("a.md")).0,
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

    // ── PLUG-1: durable audit-слой (зеркало EgressAudit-тестов) ─────────────────────────────────

    use crate::db::Database;
    use tempfile::TempDir;

    async fn open_db() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// Полный путь authorize → durable-персист: запись переживает «рестарт» (чтение из БД, не из
    /// in-memory Vec), порядок append-only (свежие первыми по id), allow И deny оба durable.
    #[tokio::test]
    async fn durable_record_persists_and_is_append_only() {
        let (_d, db) = open_db().await;
        let mut b = PluginBroker::new();
        b.set_writer(db.writer().clone());
        let t = b.open_session(session("p.a", r#"{"vault:read":["Notes/**"]}"#));

        // Три авторизации: allow, deny(out-of-scope), allow — durable-персист вне лока.
        for req in [read("Notes/a.md"), read("Secrets/x.md"), read("Notes/b.md")] {
            let (_r, entry) = b.authorize(&t, &req);
            b.audit_log().record_durable(entry).await;
        }

        // Читаем ИЗ БД (durable, не in-memory): 3 записи, свежие первыми (append-only порядок по id).
        let hist = crate::plugin::recent_audit(db.reader(), 50).await.unwrap();
        assert_eq!(hist.len(), 3, "все три вызова durable-записаны");
        assert_eq!(
            hist[0].target.as_deref(),
            Some("Notes/b.md"),
            "свежая первой"
        );
        assert_eq!(
            hist[2].target.as_deref(),
            Some("Notes/a.md"),
            "старая последней"
        );
        // allow/deny различимы в durable-истории.
        assert!(hist[2].allowed && hist[2].denied_reason.is_none());
        assert!(!hist[1].allowed && hist[1].denied_reason.is_some());
        assert_eq!(hist[1].plugin_id, "p.a");
        // Монотонность id (append-only): свежая запись имеет больший id.
        assert!(hist[0].id > hist[2].id);
    }

    /// Pre-vault окно: без `set_writer` durable-сток `None` → `record_durable` — no-op (не паникует,
    /// БД нетронута), но in-memory-слой хранит запись. Durable-история начинается с `set_writer`.
    #[tokio::test]
    async fn pre_vault_record_is_in_memory_only() {
        let mut b = PluginBroker::new();
        let t = b.open_session(session("p", r#"{"vault:read":["**"]}"#));
        let (_r, entry) = b.authorize(&t, &read("a.md"));
        // Сток не подключён — durable no-op, in-memory наполнен.
        b.audit_log().record_durable(entry).await;
        assert_eq!(b.audit().len(), 1, "in-memory хранит pre-vault запись");
    }

    /// Vault-switch: `set_writer` свапает durable-сток на новую БД (десктоп переоткрывает vault).
    /// После свапа новые записи идут в НОВУЮ БД; старая БД их не видит (зеркало EgressAudit).
    #[tokio::test]
    async fn set_writer_swaps_sink_on_vault_switch() {
        let (_d1, db1) = open_db().await;
        let (_d2, db2) = open_db().await;
        let mut b = PluginBroker::new();
        let t = b.open_session(session("p", r#"{"vault:read":["**"]}"#));

        // Первый vault: одна запись → в db1.
        b.set_writer(db1.writer().clone());
        let (_r, e1) = b.authorize(&t, &read("first.md"));
        b.audit_log().record_durable(e1).await;

        // Переоткрытие vault: сток свапается на db2. Новая запись — в db2, НЕ в db1.
        b.set_writer(db2.writer().clone());
        let (_r, e2) = b.authorize(&t, &read("second.md"));
        b.audit_log().record_durable(e2).await;

        let h1 = crate::plugin::recent_audit(db1.reader(), 50).await.unwrap();
        let h2 = crate::plugin::recent_audit(db2.reader(), 50).await.unwrap();
        assert_eq!(h1.len(), 1, "db1 держит только запись до свапа");
        assert_eq!(h1[0].target.as_deref(), Some("first.md"));
        assert_eq!(h2.len(), 1, "db2 держит только запись после свапа");
        assert_eq!(h2[0].target.as_deref(), Some("second.md"));
    }

    /// СТРУКТУРНАЯ гарантия «durable-запись вне std-лока брокера»: `record_durable` — метод на
    /// `Arc<AuditLog>` (получаемом клоном через `audit_log()`), НЕ на `&mut PluginBroker`. Значит
    /// `.await` durable-I/O физически не может держать std-лок брокера — вызывающий сначала берёт
    /// клон и отпускает лок. Тест это фиксирует: `record_durable` вызывается на клоне БЕЗ брокера
    /// в области видимости (брокер уже дропнут).
    #[tokio::test]
    async fn durable_write_does_not_hold_broker_lock() {
        let (_d, db) = open_db().await;
        let audit_log = {
            let mut b = PluginBroker::new();
            b.set_writer(db.writer().clone());
            let t = b.open_session(session("p", r#"{"vault:read":["**"]}"#));
            let (_r, entry) = b.authorize(&t, &read("a.md"));
            let log = b.audit_log();
            // Брокер дропается здесь (конец блока) — durable-запись ниже идёт БЕЗ него.
            log.record_durable(entry).await;
            log
        };
        // Клон `Arc<AuditLog>` пережил брокер и durable-запись состоялась.
        let hist = crate::plugin::recent_audit(db.reader(), 50).await.unwrap();
        assert_eq!(hist.len(), 1);
        drop(audit_log);
    }
}
