//! host/act — RPC vault-записи песочного прогона (SANDBOX-3, спека §5.1).
//!
//! Vault в контейнере монтируется `:ro` → in-sandbox актуатор-инструменты НЕ пишут локально; они шлют
//! typed JSON-RPC `host/act` → ХОСТ исполняет НЕИЗМЕНЁННЫЙ `dispatch_action` (classify / RiskTier×autonomy
//! / TokenBucket / kill-switch / `resolve_vault_path_for_write` / snapshot / ledger write-before-act /
//! undo — всё host-side, authoritative). Зеркалит SANDBOX-2 ([`super::proxy`]): host-server + in-sandbox
//! шим + backend-трейт (Tier-1-тестируемо без рантайма).
//!
//! [`crate::actuator::Action`]/[`ActionTarget`] НЕ сериализуются напрямую — это security-keystone
//! (exhaustive-match держит classify честным). Вместо этого — wire-DTO [`WireAction`] с EXHAUSTIVE
//! fail-closed конверсией [`TryFrom<&Action>`]: Фаза-3 exec-таргеты (`ShellRun`/`ProcessSpawn`/`GitOp`)
//! НЕ представимы на `host/act` → `Err` (их путь — отдельный `host/exec`, 6c). `WireKind` знает лишь 3
//! vault-вида → контейнер СТРУКТУРНО не протолкнёт exec через host/act (forge невозможен).

use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actuator::{
    dispatch_action, Action, ActionDispatcher, ActionTarget, DispatchOutcome, GatedToolCtx,
};
use crate::agent::connect::{RpcError, RpcMessage, Transport};
use crate::agent::ToolError;

/// JSON-RPC метод: vault-запись через host-side гейт.
pub const HOST_ACT: &str = "host/act";

/// Вид действия на проводе (плоский дискриминант — `flatten`+`deny_unknown_fields` в serde конфликтуют,
/// поэтому DTO плоский: `{kind, rel, key?, content?, value?}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireKind {
    NoteCreate,
    NoteEdit,
    Frontmatter,
}

/// Wire-DTO действия (≠ `actuator::Action` — security-keystone не сериализуем). `deny_unknown_fields` —
/// fail-closed как у tool-арг (`PathContentArgs`): лишнее поле → отказ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireAction {
    pub kind: WireKind,
    /// vault-rel путь цели.
    pub rel: String,
    /// Ключ frontmatter (только `Frontmatter`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Тело (NoteCreate/NoteEdit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Значение ключа (Frontmatter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl TryFrom<&Action> for WireAction {
    type Error = &'static str;
    /// FAIL-CLOSED (Фаза-3 keystone): `host/act` НЕСЁТ ТОЛЬКО vault-таргеты. exec-таргеты
    /// (`ShellRun`/`ProcessSpawn`/`GitOp`) НЕ представимы на проводе host/act → `Err` (их путь — отдельный
    /// host/exec, Фаза-3 6c). EXHAUSTIVE (без `_ =>`): новый ActionTarget-вариант осознанно решит, vault он
    /// или нет. Так контейнер СТРУКТУРНО не может протолкнуть exec через host/act (forge невозможен —
    /// WireKind знает лишь 3 vault-вида).
    fn try_from(a: &Action) -> Result<Self, Self::Error> {
        let (kind, rel, key) = match &a.target {
            ActionTarget::NoteCreate { rel } => (WireKind::NoteCreate, rel.clone(), None),
            ActionTarget::NoteEdit { rel } => (WireKind::NoteEdit, rel.clone(), None),
            ActionTarget::Frontmatter { rel, key } => {
                (WireKind::Frontmatter, rel.clone(), Some(key.clone()))
            }
            ActionTarget::ShellRun { .. }
            | ActionTarget::ProcessSpawn { .. }
            | ActionTarget::GitOp { .. } => {
                return Err("exec-таргет не представим на host/act (используй host/exec, Фаза-3)")
            }
            // SL-7: SkillSave не представим на sandbox-wire (v1 — только in-process; навыки регистрируются
            // лишь в session.rs, НЕ в sandbox/child.rs). Forge-невозможен: WireKind знает лишь 3 vault-вида.
            ActionTarget::SkillSave { .. } => {
                return Err("SkillSave не представим на host/act (v1 — только in-process)")
            }
        };
        Ok(WireAction {
            kind,
            rel,
            key,
            content: a.content.clone(),
            value: a.value.clone(),
        })
    }
}

impl TryFrom<WireAction> for Action {
    type Error = &'static str;
    fn try_from(w: WireAction) -> Result<Self, Self::Error> {
        let WireAction {
            kind,
            rel,
            key,
            content,
            value,
        } = w;
        let target = match kind {
            WireKind::NoteCreate => ActionTarget::NoteCreate { rel },
            WireKind::NoteEdit => ActionTarget::NoteEdit { rel },
            WireKind::Frontmatter => ActionTarget::Frontmatter {
                rel,
                key: key.ok_or("frontmatter требует key")?,
            },
        };
        Ok(Action {
            target,
            content,
            value,
        })
    }
}

/// Исход dispatch на проводе (≠ `DispatchOutcome` — держим actuator-типы без serde).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WireDispatchOutcome {
    Applied { summary: String },
    Rejected { summary: String },
    Failed { summary: String },
}

impl From<DispatchOutcome> for WireDispatchOutcome {
    fn from(o: DispatchOutcome) -> Self {
        match o {
            DispatchOutcome::Applied(s) => WireDispatchOutcome::Applied { summary: s },
            DispatchOutcome::Rejected(s) => WireDispatchOutcome::Rejected { summary: s },
            DispatchOutcome::Failed(s) => WireDispatchOutcome::Failed { summary: s },
        }
    }
}

impl From<WireDispatchOutcome> for DispatchOutcome {
    fn from(w: WireDispatchOutcome) -> Self {
        match w {
            WireDispatchOutcome::Applied { summary } => DispatchOutcome::Applied(summary),
            WireDispatchOutcome::Rejected { summary } => DispatchOutcome::Rejected(summary),
            WireDispatchOutcome::Failed { summary } => DispatchOutcome::Failed(summary),
        }
    }
}

/// Абстракция host-side актуатора (за ней — НЕИЗМЕНЁННЫЙ `dispatch_action` с per-run контекстом). Вынесена
/// ради Tier-1-тестируемости `HostActServer` без vault/гейта (мок). Реальный бэкенд (`dispatch_action` +
/// policy/decision/events/ledger/canon_root прогона) собирает `SandboxRunner` — SANDBOX-4.
#[async_trait]
pub trait ActuatorBackend: Send + Sync {
    /// Исполнить действие host-side. `run_id`/контекст держит бэкенд (host-штамповка). Err(ToolError) —
    /// HardBlocked/невалид (фенсенная ошибка для агента).
    async fn act(&self, action: &Action) -> Result<DispatchOutcome, ToolError>;
}

/// РЕАЛЬНЫЙ host-side [`ActuatorBackend`] рантайма песочницы (SANDBOX-4b): держит [`GatedToolCtx`]
/// прогона (ВСЕ deps `dispatch_action` — canon_root / ledger / run_id / policy / decision_source / events)
/// и исполняет действие через НЕИЗМЕНЁННЫЙ `dispatch_action`. Это host-сторона `host/act`: in-sandbox
/// [`ProxyActuator`] шлёт RPC → [`HostActServer`] десериализует [`WireAction`]→`Action` → ЭТОТ бэкенд
/// применяет authoritative (classify / RiskTier×autonomy / TokenBucket / kill-switch / snapshot / ledger
/// write-before-act / undo — всё host-side).
///
/// КЛЮЧЕВОЙ ИНВАРИАНТ (нет второго policy-пути): `GatedToolCtx` — РОВНО тот контекст, что несут in-process
/// актуатор-инструменты ([`crate::actuator::NoteCreateTool`] и пр.), поэтому гейт/леджер/blast-radius/undo
/// у песочного прогона ИДЕНТИЧНЫ in-process пути. Песочница лишь добавляет OS-изоляцию вокруг loop'а;
/// authoritative-решение остаётся в ОДНОМ `dispatch_action`. Proposal/Diff эмитятся `events` контекста
/// (host-side forwarder → десктоп), а decision приходит host-side (`decision_source`) — НЕ из контейнера.
pub struct DispatchActuatorBackend {
    ctx: GatedToolCtx,
}

impl DispatchActuatorBackend {
    /// Собрать из per-run [`GatedToolCtx`] (тот же, что у in-process инструментов прогона).
    pub fn new(ctx: GatedToolCtx) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ActuatorBackend for DispatchActuatorBackend {
    async fn act(&self, action: &Action) -> Result<DispatchOutcome, ToolError> {
        dispatch_action(
            action,
            self.ctx.run_id,
            &self.ctx.policy,
            &self.ctx.decision_source,
            self.ctx.events.as_ref(),
            self.ctx.ledger.as_ref(),
            self.ctx.canon_root.as_path(),
        )
        .await
    }
}

/// Host-side обработчик `host/act`: десериализует [`WireAction`] → `Action` (exhaustive fail-closed) →
/// бэкенд (`dispatch_action`) → [`WireDispatchOutcome`]. HardBlocked/ошибка → `Failed{summary}` (та же
/// фенсенная граница, что у in-process инструмента: агент видит причину и переспрашивает).
pub struct HostActServer<B: ActuatorBackend> {
    backend: B,
}

impl<B: ActuatorBackend> HostActServer<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// Обрабатывает один `host/act`-запрос. `Ok(Value)` = сериализованный [`WireDispatchOutcome`].
    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        if method != HOST_ACT {
            return Err(RpcError::method_not_found());
        }
        let wire: WireAction =
            serde_json::from_value(params).map_err(|_| RpcError::invalid_params())?;
        let action: Action = wire.try_into().map_err(|_| RpcError::invalid_params())?;
        let outcome = match self.backend.act(&action).await {
            Ok(o) => WireDispatchOutcome::from(o),
            // ToolError (HardBlocked/невалид) → Failed: текст — про vault-действие (не секрет), полезен
            // агенту для реплана; tool-граница свернёт Failed → Err так же, как in-process.
            Err(e) => WireDispatchOutcome::Failed {
                summary: e.to_string(),
            },
        };
        serde_json::to_value(outcome).map_err(|e| RpcError::internal(e.to_string()))
    }
}

/// In-sandbox-шим: фреймит `host/act` поверх [`Transport`] к host-side [`HostActServer`]. Используется
/// актуатор-инструментами контейнера ВМЕСТО локального `dispatch_action` (vault `:ro`). Возвращает
/// [`DispatchOutcome`] (инструмент свернёт через `into_tool_result`); транспорт-сбой → `ToolError::Exec`.
pub struct ProxyActuator<T: Transport> {
    transport: T,
    next_id: Mutex<i64>,
}

impl<T: Transport> ProxyActuator<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: Mutex::new(1),
        }
    }

    pub async fn dispatch(&self, action: &Action) -> Result<DispatchOutcome, ToolError> {
        let id = {
            let mut g = self.next_id.lock().expect("id mutex");
            let id = *g;
            *g += 1;
            id
        };
        // FAIL-CLOSED: exec-таргет не представим на host/act (Фаза-3 → host/exec). TryFrom→Err → ToolError.
        let wire = WireAction::try_from(action).map_err(|e| ToolError::Exec(e.to_string()))?;
        let params = serde_json::to_value(wire)
            .map_err(|e| ToolError::Exec(format!("host/act сериализация: {e}")))?;
        self.transport
            .send(RpcMessage::request(id, HOST_ACT, params))
            .await
            .map_err(|_| ToolError::Exec("host/act транспорт (send)".into()))?;
        let msg = self
            .transport
            .recv()
            .await
            .ok_or_else(|| ToolError::Exec("host/act транспорт закрыт".into()))?;
        match msg {
            RpcMessage::Response {
                id: resp_id,
                result,
            } => {
                if resp_id != id {
                    return Err(ToolError::Exec("host/act: id ответа не совпал".into()));
                }
                match result {
                    Ok(v) => {
                        let wire: WireDispatchOutcome = serde_json::from_value(v)
                            .map_err(|e| ToolError::Exec(format!("host/act ответ: {e}")))?;
                        Ok(DispatchOutcome::from(wire))
                    }
                    // RpcError (invalid_params/method) — фенсенная ошибка инструменту.
                    Err(e) => Err(ToolError::Exec(format!("host/act отказ: {}", e.message))),
                }
            }
            _ => Err(ToolError::Exec("host/act: ожидался Response".into())),
        }
    }
}

/// In-sandbox реализация [`ActionDispatcher`] (ШОВ актуатора): файловые инструменты в контейнере держат
/// `Arc<dyn ActionDispatcher>` = `Arc<ProxyActuator>` → каждое применение уходит `host/act` RPC хосту
/// (vault `:ro` в контейнере, authoritative-гейт host-side). Свёртка идентична in-process пути
/// ([`GatedToolCtx::apply`]): Applied/Rejected → `Ok(summary)`, Failed/HardBlock → `Err(ToolError)`.
#[async_trait]
impl<T: Transport> ActionDispatcher for ProxyActuator<T> {
    async fn apply(&self, action: Action) -> Result<String, ToolError> {
        self.dispatch(&action).await?.into_tool_result()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::connect::channel_pair;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn wire_action_roundtrip_all_targets() {
        let actions = [
            Action::note_create("Notes/A.md", "body-a"),
            Action::note_edit("Notes/B.md", "body-b"),
            Action::frontmatter("Notes/C.md", "status", "done"),
        ];
        for a in actions {
            let wire = WireAction::try_from(&a).unwrap();
            let json = serde_json::to_string(&wire).unwrap();
            let back: WireAction = serde_json::from_str(&json).unwrap();
            let a2: Action = back.try_into().unwrap();
            assert_eq!(a, a2, "round-trip Action↔WireAction↔JSON");
        }
    }

    /// Фаза-3 keystone: exec-таргеты НЕ представимы на host/act (TryFrom→Err); vault-таргеты → Ok.
    #[test]
    fn exec_action_not_representable_on_host_act() {
        for a in [
            Action::shell_run(vec!["ls".into()], None),
            Action::process_spawn("git", vec!["status".into()], None),
            Action::git_op("status", vec![]),
        ] {
            assert!(
                WireAction::try_from(&a).is_err(),
                "exec не на host/act: {a:?}"
            );
        }
        assert!(WireAction::try_from(&Action::note_create("A.md", "b")).is_ok());
    }

    #[test]
    fn wire_action_rejects_unknown_field() {
        let json = r#"{"kind":"note_create","rel":"X.md","content":"y","bogus":1}"#;
        assert!(serde_json::from_str::<WireAction>(json).is_err());
    }

    #[test]
    fn frontmatter_without_key_is_rejected() {
        let wire = WireAction {
            kind: WireKind::Frontmatter,
            rel: "X.md".into(),
            key: None,
            content: None,
            value: Some("v".into()),
        };
        assert!(Action::try_from(wire).is_err());
    }

    // --- РЕАЛЬНЫЙ бэкенд (DispatchActuatorBackend) end-to-end (Tier-1: настоящий vault + dispatch_action) ---

    use crate::actuator::{AuditSink, DecisionSource, DispatchPolicy, PolicyDefault};
    use crate::db::Database;
    use tempfile::TempDir;

    /// Временный КАНОНИЗИРОВАННЫЙ vault + БД + `GatedToolCtx` (auto-политика, токены есть → Auto-тир
    /// применяется сразу). Зеркалит harness `orchestrate::tests::setup`.
    async fn real_gate(autonomy: Option<&str>) -> (TempDir, std::path::PathBuf, GatedToolCtx) {
        let dir = TempDir::new().unwrap();
        let canon_root = dir.path().canonicalize().unwrap();
        let db = Database::open(canon_root.join(".nexus/nexus.db"))
            .await
            .unwrap();
        let ledger = AuditSink::new(db.writer().clone(), db.reader().clone());
        std::mem::forget(db); // writer/reader клонированы в ledger — актор жив, пока жив клон.
        let policy = DispatchPolicy::new(autonomy, 100, 3);
        let decision: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
        let events: Arc<dyn crate::actuator::EventSink> =
            Arc::new(crate::actuator::CollectingSink::new());
        let ctx = GatedToolCtx::new(canon_root.clone(), ledger, 1, policy, decision, events);
        (dir, canon_root, ctx)
    }

    /// Реальный бэкенд + auto-политика ⇒ Auto-тир `note.create` ПРИМЕНЯЕТСЯ на диск (тот же
    /// `dispatch_action`, что in-process) — доказывает, что `GatedToolCtx`→`dispatch_action` живёт
    /// через `ActuatorBackend`-трейт без второго policy-пути.
    #[tokio::test]
    async fn dispatch_backend_applies_auto_tier_to_disk() {
        let (_d, root, ctx) = real_gate(Some("auto")).await;
        let backend = DispatchActuatorBackend::new(ctx);
        let out = backend
            .act(&Action::note_create("Notes/Real.md", "живое тело"))
            .await
            .unwrap();
        assert!(matches!(out, DispatchOutcome::Applied(_)), "out={out:?}");
        assert_eq!(
            std::fs::read_to_string(root.join("Notes/Real.md")).unwrap(),
            "живое тело"
        );
    }

    /// ПОЛНЫЙ host-путь песочницы: `WireAction` → `HostActServer` → РЕАЛЬНЫЙ `DispatchActuatorBackend`
    /// → `dispatch_action` → запись на диск. Это сборка SANDBOX-3 (сервер) + SANDBOX-4b (реальный бэкенд).
    #[tokio::test]
    async fn host_act_server_with_real_backend_writes_to_disk() {
        let (_d, root, ctx) = real_gate(Some("auto")).await;
        let srv = HostActServer::new(DispatchActuatorBackend::new(ctx));
        let params = serde_json::to_value(
            WireAction::try_from(&Action::note_create("Notes/Wire.md", "из RPC")).unwrap(),
        )
        .unwrap();
        let out = srv.handle(HOST_ACT, params).await.unwrap();
        let w: WireDispatchOutcome = serde_json::from_value(out).unwrap();
        assert!(matches!(w, WireDispatchOutcome::Applied { .. }), "w={w:?}");
        assert_eq!(
            std::fs::read_to_string(root.join("Notes/Wire.md")).unwrap(),
            "из RPC"
        );
    }

    /// **ПОЛНАЯ ЦЕПЬ ПЕСОЧНОГО АКТУАТОРА через `Tool`-трейт + ШОВ `ActionDispatcher`**: in-sandbox
    /// `NoteCreateTool` держит `Arc<ProxyActuator>` (как соберёт `--sandbox-child`) → `invoke` → `host/act`
    /// RPC → `HostActServer` → `DispatchActuatorBackend` → `dispatch_action` → запись на диск. Доказывает,
    /// что инструмент НЕ знает про транспорт (тот же `NoteCreateTool`, что in-process), а запись
    /// authoritative host-side.
    #[tokio::test]
    async fn sandbox_tool_via_proxy_actuator_writes_through_host() {
        use crate::actuator::NoteCreateTool;
        use crate::agent::Tool;
        use std::sync::Arc;

        let (client_t, host_t) = channel_pair();
        let (_d, root, ctx) = real_gate(Some("auto")).await;
        let srv = HostActServer::new(DispatchActuatorBackend::new(ctx));
        // Host обслуживает один host/act-запрос (как сделает act.sock-сервер рантайма).
        let host = tokio::spawn(async move {
            if let Some(RpcMessage::Request { id, method, params }) = host_t.recv().await {
                let result = srv.handle(&method, params).await;
                host_t
                    .send(RpcMessage::Response { id, result })
                    .await
                    .unwrap();
            }
        });

        // In-sandbox инструмент: тот же NoteCreateTool, но диспетчер — ProxyActuator (host/act RPC).
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(ProxyActuator::new(client_t));
        let tool = NoteCreateTool::new(dispatcher);
        let res = tool
            .invoke(r#"{"path":"Notes/Sbx.md","content":"через песочницу"}"#)
            .await
            .unwrap();

        assert!(res.contains("создана"), "резюме apply: {res}");
        assert_eq!(
            std::fs::read_to_string(root.join("Notes/Sbx.md")).unwrap(),
            "через песочницу",
            "запись прошла host-side через host/act"
        );
        host.await.unwrap();
    }

    /// confirm-политика + `PolicyDefault` (reject-all) ⇒ host-бэкенд НЕ пишет (Confirm/Auto-предложение
    /// отклонено host-side) — kill-path: контейнер НЕ может форсировать запись, host решает.
    #[tokio::test]
    async fn dispatch_backend_confirm_policy_does_not_write() {
        let (_d, root, ctx) = real_gate(Some("confirm")).await;
        let backend = DispatchActuatorBackend::new(ctx);
        let out = backend
            .act(&Action::note_create(
                "Notes/Nope.md",
                "не должно записаться",
            ))
            .await
            .unwrap();
        assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
        assert!(!root.join("Notes/Nope.md").exists(), "файл НЕ записан");
    }

    /// Мок-бэкенд: записывает последнее действие + возвращает заданный исход (без vault/гейта).
    struct MockBackend {
        calls: AtomicUsize,
        last_rel: Mutex<String>,
        outcome: Mutex<Option<Result<DispatchOutcome, ToolError>>>,
    }
    impl MockBackend {
        fn new(o: Result<DispatchOutcome, ToolError>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                last_rel: Mutex::new(String::new()),
                outcome: Mutex::new(Some(o)),
            })
        }
    }
    #[async_trait]
    impl ActuatorBackend for Arc<MockBackend> {
        async fn act(&self, action: &Action) -> Result<DispatchOutcome, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_rel.lock().unwrap() = action.target.rel().to_string();
            self.outcome.lock().unwrap().take().unwrap()
        }
    }

    #[tokio::test]
    async fn host_act_server_maps_applied() {
        let mock = MockBackend::new(Ok(DispatchOutcome::Applied("ok".into())));
        let srv = HostActServer::new(mock.clone());
        let params = serde_json::to_value(
            WireAction::try_from(&Action::note_create("Notes/A.md", "b")).unwrap(),
        )
        .unwrap();
        let out = srv.handle(HOST_ACT, params).await.unwrap();
        let w: WireDispatchOutcome = serde_json::from_value(out).unwrap();
        assert_eq!(
            w,
            WireDispatchOutcome::Applied {
                summary: "ok".into()
            }
        );
        assert_eq!(&*mock.last_rel.lock().unwrap(), "Notes/A.md");
    }

    #[tokio::test]
    async fn host_act_server_maps_toolerror_to_failed() {
        let mock = MockBackend::new(Err(ToolError::Exec("hardblocked".into())));
        let srv = HostActServer::new(mock);
        let params =
            serde_json::to_value(WireAction::try_from(&Action::note_edit("X.md", "b")).unwrap())
                .unwrap();
        let out = srv.handle(HOST_ACT, params).await.unwrap();
        let w: WireDispatchOutcome = serde_json::from_value(out).unwrap();
        assert!(
            matches!(w, WireDispatchOutcome::Failed { summary } if summary.contains("hardblocked"))
        );
    }

    #[tokio::test]
    async fn host_act_unknown_method_not_found() {
        let mock = MockBackend::new(Ok(DispatchOutcome::Applied("x".into())));
        let srv = HostActServer::new(mock);
        assert!(srv.handle("host/exec", Value::Null).await.is_err());
    }

    #[tokio::test]
    async fn proxy_actuator_roundtrip_over_channel() {
        let (client_t, host_t) = channel_pair();
        let mock = MockBackend::new(Ok(DispatchOutcome::Applied("создано".into())));
        let srv = HostActServer::new(mock.clone());
        let host = tokio::spawn(async move {
            let msg = host_t.recv().await.unwrap();
            if let RpcMessage::Request { id, method, params } = msg {
                let result = srv.handle(&method, params).await;
                host_t
                    .send(RpcMessage::Response { id, result })
                    .await
                    .unwrap();
            }
        });
        let shim = ProxyActuator::new(client_t);
        let outcome = shim
            .dispatch(&Action::note_create("Notes/Z.md", "тело"))
            .await
            .unwrap();
        assert_eq!(outcome, DispatchOutcome::Applied("создано".into()));
        // tool-граница: Applied → Ok(summary).
        assert_eq!(outcome.into_tool_result().unwrap(), "создано");
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
        host.await.unwrap();
    }

    #[tokio::test]
    async fn proxy_actuator_folds_failed_to_tool_error() {
        let (client_t, host_t) = channel_pair();
        let mock = MockBackend::new(Err(ToolError::Exec("путь вне vault".into())));
        let srv = HostActServer::new(mock);
        let host = tokio::spawn(async move {
            let msg = host_t.recv().await.unwrap();
            if let RpcMessage::Request { id, method, params } = msg {
                let result = srv.handle(&method, params).await;
                host_t
                    .send(RpcMessage::Response { id, result })
                    .await
                    .unwrap();
            }
        });
        let shim = ProxyActuator::new(client_t);
        let outcome = shim
            .dispatch(&Action::note_edit("../escape.md", "x"))
            .await
            .unwrap();
        // Failed → tool-граница свернёт в Err(ToolError::Exec) — как in-process HardBlock.
        assert!(outcome.into_tool_result().is_err());
        host.await.unwrap();
    }
}
