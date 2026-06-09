//! Egress-граница ядра (ADR-005-ext, решения E1–E10; `docs/dev/net.md`, срез 1 «Фундамент»).
//!
//! ЕДИНСТВЕННЫЙ chokepoint исходящего HTTP ядра: каждый core-эгресс (chat/embed/probe, в будущем
//! web/cloud/News Feed) обязан идти через [`GuardedClient`]. Голый `reqwest::Client::builder` /
//! `core_client_builder` вне `net/` запрещён CI-grep-линтом (`scripts/check-egress.mjs`, AC-EGR-1).
//!
//! Порядок проверки [`EgressPolicy::check`] per-request (AC-EGR-2/3/5/12):
//! metadata-блок (всегда) → kill-switch «офлайн» (рубит публичные, LAN/loopback живут, E2) →
//! per-feature opt-in (E6) → host ∈ allowlist ИЛИ `is_private_host` (local-first; E4: хосты из
//! `local.json ai.*` авто-в-allowlist). Каждый вызов — успех И отказ — пишется в неотключаемый
//! append-only [`EgressAudit`] (E8, AC-EGR-4); host — через [`Redacted`] (значение не утекает в Debug).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use thiserror::Error;

use crate::plugin::{blocks_cloud_metadata, is_private_host};
use crate::redact::Redacted;

/// Общий конструктор HTTP-клиента ядра для LLM-серверов (chat/embedding): **не следует редиректам**
/// (анти-SSRF, AC-SEC-4 / ревью C5). Скомпрометированный или подменённый эндпоинт не может 30x-редиректом
/// увести запрос ядра на внутренний/metadata-адрес. Таймауты задаёт вызывающий.
///
/// Приватная деталь `net/` (AC-EGR-7): снаружи клиент ядра строится ТОЛЬКО через [`GuardedClient`];
/// вызов этого билдера вне `net/` ловит CI-grep-линт (AC-EGR-1).
fn core_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
}

/// Сетевая фича ядра — ось политики и audit. `Web`/`NewsFeed`/`CloudFallback` добавляются ВМЕСТЕ со
/// своими фичами (срезы 3–4), не впрок: durability-замок — chokepoint + grep-линт, а не этот enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressFeature {
    Chat,
    Embed,
    Probe,
}

impl EgressFeature {
    /// Индекс в таблице opt-in-флагов политики.
    fn idx(self) -> usize {
        match self {
            EgressFeature::Chat => 0,
            EgressFeature::Embed => 1,
            EgressFeature::Probe => 2,
        }
    }
}

impl std::fmt::Display for EgressFeature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EgressFeature::Chat => "chat",
            EgressFeature::Embed => "embed",
            EgressFeature::Probe => "probe",
        })
    }
}

/// Структурированный отказ политики эгресса (AC-EGR-14: типизированная причина, не reqwest-строка;
/// i18n-рендер на фронте — отдельный срез). Хост — в [`Redacted`]: в Display/Debug не печатается.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EgressDenied {
    /// Kill-switch «офлайн»: публичные хосты отрезаны, LAN/loopback живут (E2, AC-EGR-3).
    #[error("офлайн-режим: исходящие запросы ядра к публичным хостам отключены")]
    Offline,
    /// Per-feature opt-in (E6, AC-EGR-5): фича не включена.
    #[error("сетевая фича «{0}» не включена")]
    FeatureNotEnabled(EgressFeature),
    /// Хост не разрешён: не в allowlist и не приватный/loopback (AC-EGR-2) либо metadata (AC-EGR-12).
    #[error("хост не разрешён политикой эгресса ядра")]
    HostNotAllowed(Redacted<String>),
}

/// Ошибка guarded-запроса: отказ политики ЛИБО транспорт. Отказ типизирован отдельно, чтобы
/// вызывающие могли отличить «политика не пустила» (ДО сокета) от сетевой ошибки.
#[derive(Debug, Error)]
pub enum NetError {
    #[error(transparent)]
    Denied(#[from] EgressDenied),
    /// URL не парсится или без хоста — в текст не включаем (приватность, AC-SEC-6).
    #[error("некорректный URL эгресса")]
    BadUrl,
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
}

/// Политика эгресса ядра. ОДИН экземпляр на приложение (composition-root, AC-EGR-13): kill-switch
/// делит `Arc<AtomicBool>` с `AppState` (E10), allowlist обновляется из `local.json` (E4).
pub struct EgressPolicy {
    /// Kill-switch «офлайн» (E2): тот же атомик, что и `AppState::egress_offline`.
    offline: Arc<AtomicBool>,
    /// Per-feature opt-in (E6), индекс — [`EgressFeature::idx`]. Chat/Embed/Probe — local-first,
    /// включены по умолчанию (`net.md`: opt-in-состояния для LAN в фундаменте нет).
    features: [AtomicBool; 3],
    /// Exact-host allowlist (как net-allowlist брокера): явные хосты из `local.json ai.*` (E4).
    /// `RwLock` — частые читатели per-request, редкая замена на open-vault/смене настроек.
    allowlist: RwLock<HashSet<String>>,
}

impl EgressPolicy {
    /// Политика с дефолтами фундамента: фичи включены (local-first), allowlist пуст (fail-closed
    /// для публичных хостов, пока composition-root не положил явные `ai.*`-хосты из `local.json`).
    pub fn new(offline: Arc<AtomicBool>) -> Self {
        Self {
            offline,
            features: [
                AtomicBool::new(true),
                AtomicBool::new(true),
                AtomicBool::new(true),
            ],
            allowlist: RwLock::new(HashSet::new()),
        }
    }

    /// Проверка per-request (порядок — дизайн-инвариант 2, `net.md`). Возвращает структурированный
    /// отказ ДО любых сетевых действий (сокет/DNS) — вызывающий обязан не отправлять запрос.
    pub fn check(&self, host: &str, feature: EgressFeature) -> Result<(), EgressDenied> {
        // 1. Cloud-metadata — БЕЗУСЛОВНО первым (E7, AC-EGR-12): ни allowlist, ни «приватность»
        //    link-local не открывают IMDS.
        if blocks_cloud_metadata(host) {
            return Err(EgressDenied::HostNotAllowed(Redacted::new(
                host.to_string(),
            )));
        }
        // 2. Kill-switch «офлайн» (E2, AC-EGR-3): рубит публичные хосты; LAN/loopback живут.
        if self.offline.load(Ordering::Relaxed) && !is_private_host(host) {
            return Err(EgressDenied::Offline);
        }
        // 3. Per-feature opt-in (E6, AC-EGR-5).
        if !self.features[feature.idx()].load(Ordering::Relaxed) {
            return Err(EgressDenied::FeatureNotEnabled(feature));
        }
        // 4. Хост: приватный/loopback (local-first, AC-EGR-9) ИЛИ явный allowlist (E4, AC-EGR-2).
        let allowed = is_private_host(host)
            || self
                .allowlist
                .read()
                .map(|a| a.contains(host))
                .unwrap_or(false); // poisoned lock → fail-closed
        if allowed {
            Ok(())
        } else {
            Err(EgressDenied::HostNotAllowed(Redacted::new(
                host.to_string(),
            )))
        }
    }

    /// Включает/выключает сетевую фичу (E6). В фундаменте дергается тестами; UI — срез 2.
    pub fn set_feature_enabled(&self, feature: EgressFeature, enabled: bool) {
        self.features[feature.idx()].store(enabled, Ordering::Relaxed);
    }

    /// Заменяет allowlist целиком (E4: пересобирается из `local.json ai.*` на open-vault и при
    /// смене настроек). Consent на pull-changed URL — срез 2 (нужен персист политики, E5).
    pub fn set_allowlist(&self, hosts: impl IntoIterator<Item = String>) {
        if let Ok(mut a) = self.allowlist.write() {
            *a = hosts.into_iter().collect();
        }
    }
}

/// Запись audit-журнала эгресса (E8, AC-EGR-4): ось `{feature, host, bytes_out?, decision}` —
/// ОТДЕЛЬНЫЙ тип от брокерского `AuditEntry` (другая ось: plugin_id/method/target). Без URL/тела.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressAuditEntry {
    pub feature: EgressFeature,
    /// Хост назначения; [`Redacted`] — не утекает в Debug/логи (AC-SEC-6).
    pub host: Redacted<String>,
    /// Размер тела ЗАПРОСА (не ответа), best-effort (AC-EGR-10): `Some` для JSON-post, `None` для GET.
    pub bytes_out: Option<usize>,
    pub allowed: bool,
    pub denied_reason: Option<String>,
}

/// Неотключаемый append-only журнал эгресса ядра (инвариант — как брокерский `AuditLog`):
/// `record()` приватен для `net/`, публичны только чтения; чистить нельзя by design (AC-EGR-4).
#[derive(Debug, Default)]
pub struct EgressAudit {
    /// `Mutex` — записи короткие и синхронные, журнал делится между провайдерами через `Arc`.
    entries: Mutex<Vec<EgressAuditEntry>>,
}

impl EgressAudit {
    /// Единственная точка записи — зовётся ТОЛЬКО из [`GuardedClient`] (приватность = append-only).
    fn record(
        &self,
        feature: EgressFeature,
        host: String,
        bytes_out: Option<usize>,
        decision: &Result<(), EgressDenied>,
    ) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(EgressAuditEntry {
                feature,
                host: Redacted::new(host),
                bytes_out,
                allowed: decision.is_ok(),
                denied_reason: decision.as_ref().err().map(|d| d.to_string()),
            });
        }
    }

    /// Снимок журнала (копия: журнал живёт под мьютексом, наружу — без ссылки на внутренности).
    pub fn entries(&self) -> Vec<EgressAuditEntry> {
        self.entries.lock().map(|e| e.clone()).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Guarded HTTP-клиент ядра — ЕДИНСТВЕННАЯ дверь исходящего HTTP (E1, AC-EGR-1). Каждый запрос:
/// `policy.check` (отказ ДО сокета/DNS) → запись в audit (успех И отказ) → реальный I/O.
/// Клонирование дёшево (`reqwest::Client` внутри — `Arc`).
#[derive(Clone)]
pub struct GuardedClient {
    inner: reqwest::Client,
    policy: Arc<EgressPolicy>,
    audit: Arc<EgressAudit>,
}

impl GuardedClient {
    /// Строит guarded-клиент поверх приватного `core_client_builder` (redirect=none сохраняется,
    /// AC-EGR-7); `tune` добавляет таймауты вызывающего, политику редиректов не трогать.
    pub fn new(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        tune: impl FnOnce(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
    ) -> Result<Self, NetError> {
        let inner = tune(core_client_builder()).build()?;
        Ok(Self {
            inner,
            policy,
            audit,
        })
    }

    /// Профиль chat-стрима: общего таймаута нет (стрим долгий, idle-таймаут — у провайдера),
    /// connect-timeout страхует от зависшего коннекта (как было в `OpenAiChatProvider::new`).
    pub fn for_chat(policy: Arc<EgressPolicy>, audit: Arc<EgressAudit>) -> Result<Self, NetError> {
        Self::new(policy, audit, |b| {
            b.connect_timeout(Duration::from_secs(15))
        })
    }

    /// Профиль эмбеддинга: общий таймаут 60 с (батчи бывают тяжёлые).
    pub fn for_embedding(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
    ) -> Result<Self, NetError> {
        Self::new(policy, audit, |b| b.timeout(Duration::from_secs(60)))
    }

    /// Профиль probe (проба размерности / «Проверить связь»): короткий таймаут вызывающего.
    pub fn for_probe(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        timeout: Duration,
    ) -> Result<Self, NetError> {
        Self::new(policy, audit, |b| b.timeout(timeout))
    }

    /// GET через политику (probe `/v1/models`). `bytes_out=None` — тела запроса нет (AC-EGR-10).
    pub async fn get(
        &self,
        url: &str,
        feature: EgressFeature,
    ) -> Result<reqwest::Response, NetError> {
        self.authorize(url, feature, None)?;
        Ok(self.inner.get(url).send().await?)
    }

    /// POST JSON-тела через политику. `bytes_out=Some(len)` — длина сериализованного тела ЗАПРОСА
    /// известна и для стрим-ответа (AC-EGR-10: best-effort, тело запроса, не ответ).
    pub async fn post_json(
        &self,
        url: &str,
        feature: EgressFeature,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, NetError> {
        let bytes = serde_json::to_vec(body).expect("serde_json::Value сериализуем всегда");
        self.authorize(url, feature, Some(bytes.len()))?;
        Ok(self
            .inner
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await?)
    }

    /// Доступ к политике (фасад `AIClient` несёт её для hot-swap/индикации; AC-EGR-13).
    pub fn policy(&self) -> &Arc<EgressPolicy> {
        &self.policy
    }

    /// Проверка политики + ровно ОДНА запись audit на вызов (успех и отказ, AC-EGR-4) — ДО сокета.
    fn authorize(
        &self,
        url: &str,
        feature: EgressFeature,
        bytes_out: Option<usize>,
    ) -> Result<(), NetError> {
        let host = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        let Some(host) = host else {
            // URL без хоста: аудитим сырой url (redacted) с отказом и не уходим в сеть.
            self.audit.record(
                feature,
                url.to_string(),
                bytes_out,
                &Err(EgressDenied::HostNotAllowed(Redacted::new(url.to_string()))),
            );
            return Err(NetError::BadUrl);
        };
        let decision = self.policy.check(&host, feature);
        self.audit.record(feature, host, bytes_out, &decision);
        decision.map_err(NetError::from)?;
        Ok(())
    }

    /// Тест-фикстура: политика с дефолтами (фичи включены, офлайн выключен, allowlist пуст) —
    /// мок-серверы на loopback проходят как `is_private_host` без живого allowlist.
    #[cfg(test)]
    pub fn unchecked() -> Self {
        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        Self::new(policy, Arc::new(EgressAudit::default()), |b| b)
            .expect("guarded-клиент без тюнинга строится всегда")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// Политика с отдельным kill-switch-атомиком (как в `AppState`) — для тестов.
    fn policy_with_switch() -> (Arc<EgressPolicy>, Arc<AtomicBool>) {
        let offline = Arc::new(AtomicBool::new(false));
        (Arc::new(EgressPolicy::new(offline.clone())), offline)
    }

    fn guarded(policy: Arc<EgressPolicy>) -> (GuardedClient, Arc<EgressAudit>) {
        let audit = Arc::new(EgressAudit::default());
        let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();
        (client, audit)
    }

    /// Мок-сервер одного запроса: отдаёт `resp` первой принятой связи (стиль прежнего ai/mod.rs).
    fn serve_once(resp: &'static str) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        (addr, handle)
    }

    /// AC-SEC-4 / ревью C5: core-HTTP-клиент НЕ следует редиректам. Локальный сервер отдаёт 302 на
    /// metadata-адрес; клиент обязан вернуть сам 302, а не пойти по `Location` (иначе — SSRF).
    #[tokio::test]
    async fn core_client_does_not_follow_redirects() {
        let (addr, server) = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 0\r\n\r\n",
        );
        let client = core_client_builder().build().unwrap();
        let resp = client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("запрос к локальному серверу");
        assert_eq!(
            resp.status().as_u16(),
            302,
            "core-клиент НЕ должен следовать редиректу (анти-SSRF, AC-SEC-4)"
        );
        server.join().unwrap();
    }

    /// AC-EGR-7 (кейс 302→metadata через guarded): редирект не выполняется И прямой запрос на
    /// metadata отклоняется политикой ВСЕГДА (AC-EGR-12) — ещё до сокета/DNS.
    #[tokio::test]
    async fn guarded_does_not_follow_redirect_to_metadata() {
        let (addr, server) = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 0\r\n\r\n",
        );
        let (client, _) = guarded(policy_with_switch().0);
        let resp = client
            .get(&format!("http://{addr}/"), EgressFeature::Probe)
            .await
            .expect("loopback разрешён local-first");
        assert_eq!(
            resp.status().as_u16(),
            302,
            "redirect=none сохранён после рефактора"
        );
        server.join().unwrap();

        let denied = client
            .get(
                "http://169.254.169.254/latest/meta-data",
                EgressFeature::Probe,
            )
            .await;
        assert!(
            matches!(
                denied,
                Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))
            ),
            "metadata отклоняется политикой, не сетевой ошибкой: {denied:?}"
        );
    }

    /// AC-EGR-12 (E7): metadata-блок — первый и безусловный: ни allowlist, ни kill-switch-ветка
    /// «приватный хост жив» его не открывают.
    #[test]
    fn policy_rejects_metadata_unconditionally() {
        let (policy, offline) = policy_with_switch();
        policy.set_allowlist(["169.254.169.254".to_string()]);
        for off in [false, true] {
            offline.store(off, Ordering::Relaxed);
            assert!(
                matches!(
                    policy.check("169.254.169.254", EgressFeature::Chat),
                    Err(EgressDenied::HostNotAllowed(_))
                ),
                "metadata reject ВСЕГДА (offline={off})"
            );
        }
    }

    /// AC-EGR-3 (E2): «офлайн» рубит публичный хост, LAN/loopback живут (local-first).
    #[test]
    fn policy_offline_blocks_public_keeps_lan() {
        let (policy, offline) = policy_with_switch();
        offline.store(true, Ordering::Relaxed);
        assert_eq!(
            policy.check("203.0.113.7", EgressFeature::Chat),
            Err(EgressDenied::Offline)
        );
        assert_eq!(
            policy.check("api.example.com", EgressFeature::Embed),
            Err(EgressDenied::Offline)
        );
        for lan in ["127.0.0.1", "192.168.0.29", "localhost"] {
            assert_eq!(
                policy.check(lan, EgressFeature::Chat),
                Ok(()),
                "{lan} живёт при офлайн (E2, local-first)"
            );
        }
    }

    /// AC-EGR-5 (E6): выключенная фича → `FeatureNotEnabled`; другие фичи не задеты; включение
    /// возвращает доступ.
    #[test]
    fn policy_feature_opt_in_is_independent() {
        let (policy, _) = policy_with_switch();
        policy.set_feature_enabled(EgressFeature::Embed, false);
        assert_eq!(
            policy.check("127.0.0.1", EgressFeature::Embed),
            Err(EgressDenied::FeatureNotEnabled(EgressFeature::Embed))
        );
        assert_eq!(
            policy.check("127.0.0.1", EgressFeature::Chat),
            Ok(()),
            "отключение одной фичи не трогает другую (AC-EGR-5)"
        );
        policy.set_feature_enabled(EgressFeature::Embed, true);
        assert_eq!(policy.check("127.0.0.1", EgressFeature::Embed), Ok(()));
    }

    /// AC-EGR-2 (юнит): публичный хост вне allowlist → `HostNotAllowed`; в allowlist → проходит;
    /// приватные проходят без allowlist (E4/local-first).
    #[test]
    fn policy_allowlist_or_private() {
        let (policy, _) = policy_with_switch();
        assert!(matches!(
            policy.check("api.example.com", EgressFeature::Chat),
            Err(EgressDenied::HostNotAllowed(_))
        ));
        policy.set_allowlist(["api.example.com".to_string()]);
        assert_eq!(policy.check("api.example.com", EgressFeature::Chat), Ok(()));
        assert_eq!(
            policy.check("192.168.0.172", EgressFeature::Chat),
            Ok(()),
            "LAN — без allowlist (local-first)"
        );
    }

    /// AC-EGR-2 (интеграция): отказ происходит ДО сокета — мок-listener обязан НЕ принять
    /// соединение. Отказ — через выключенную фичу (любой отказ режется в одной точке authorize).
    #[tokio::test]
    async fn denied_request_never_touches_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();

        let (policy, _) = policy_with_switch();
        policy.set_feature_enabled(EgressFeature::Chat, false);
        let (client, audit) = guarded(policy);

        let res = client
            .post_json(
                &format!("http://{addr}/v1/chat/completions"),
                EgressFeature::Chat,
                &serde_json::json!({"messages": []}),
            )
            .await;
        assert!(matches!(
            res,
            Err(NetError::Denied(EgressDenied::FeatureNotEnabled(
                EgressFeature::Chat
            )))
        ));
        assert!(
            matches!(
                listener.accept(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
            ),
            "0 сетевых коннектов: listener не должен был принять соединение (AC-EGR-2)"
        );
        let entries = audit.entries();
        assert_eq!(entries.len(), 1, "ровно одна audit-запись на отказ");
        assert!(!entries[0].allowed);
    }

    /// AC-EGR-2: `HostNotAllowed` режется до DNS — иначе `.invalid`-домен дал бы сетевую
    /// (resolve) ошибку `NetError::Http`, а не структурированный отказ.
    #[tokio::test]
    async fn host_not_allowed_denied_before_dns() {
        let (client, _) = guarded(policy_with_switch().0);
        let res = client
            .get(
                "http://egress-foundation-test.invalid/v1/models",
                EgressFeature::Probe,
            )
            .await;
        assert!(
            matches!(res, Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))),
            "ожидали отказ политики ДО DNS: {res:?}"
        );
    }

    /// AC-EGR-3/9 (интеграция): при kill-switch=офлайн loopback-эгресс реально работает
    /// (локальный LLM жив), а публичный отклоняется типизированно.
    #[tokio::test]
    async fn offline_keeps_loopback_alive() {
        let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let (policy, offline) = policy_with_switch();
        offline.store(true, Ordering::Relaxed);
        let (client, _) = guarded(policy);

        let resp = client
            .get(&format!("http://{addr}/v1/models"), EgressFeature::Probe)
            .await
            .expect("loopback живёт при офлайн (E2)");
        assert_eq!(resp.status().as_u16(), 200);
        server.join().unwrap();

        let denied = client
            .get("http://203.0.113.7/", EgressFeature::Probe)
            .await;
        assert!(matches!(
            denied,
            Err(NetError::Denied(EgressDenied::Offline))
        ));
    }

    /// AC-EGR-4: успех И отказ → по одной записи `{feature, host, bytes_out?, decision}`;
    /// Debug записи НЕ печатает хост (`Redacted`); публичного мутатора/clear у журнала нет.
    #[tokio::test]
    async fn audit_records_success_and_denial_with_redacted_host() {
        let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let (policy, _) = policy_with_switch();
        let (client, audit) = guarded(policy.clone());

        client
            .get(&format!("http://{addr}/v1/models"), EgressFeature::Probe)
            .await
            .expect("loopback разрешён");
        server.join().unwrap();
        let denied = client
            .get("http://api.example.com/v1/models", EgressFeature::Probe)
            .await;
        assert!(matches!(denied, Err(NetError::Denied(_))));

        let entries = audit.entries();
        assert_eq!(entries.len(), 2, "каждый вызов — ровно одна запись");
        assert!(entries[0].allowed && entries[1].denied_reason.is_some());
        assert_eq!(entries[1].feature, EgressFeature::Probe);
        let dump = format!("{entries:?}");
        assert!(
            !dump.contains("127.0.0.1") && !dump.contains("api.example.com"),
            "host в audit — Redacted, в Debug не утекает (AC-EGR-4): {dump}"
        );
        assert_eq!(
            entries[0].host.expose(),
            "127.0.0.1",
            "явный expose() работает"
        );
    }

    /// AC-EGR-10: `bytes_out` — best-effort размер тела ЗАПРОСА: `Some(len)` для JSON-post
    /// (длина сериализованного тела), `None` для GET.
    #[tokio::test]
    async fn bytes_out_is_request_body_best_effort() {
        let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let (policy, _) = policy_with_switch();
        let (client, audit) = guarded(policy);

        let body =
            serde_json::json!({"model": "gemma", "messages": [{"role": "user", "content": "hi"}]});
        let expected = serde_json::to_vec(&body).unwrap().len();
        client
            .post_json(
                &format!("http://{addr}/v1/chat/completions"),
                EgressFeature::Chat,
                &body,
            )
            .await
            .expect("loopback разрешён");
        server.join().unwrap();
        let denied_get = client
            .get("http://api.example.com/x", EgressFeature::Probe)
            .await;
        assert!(denied_get.is_err());

        let entries = audit.entries();
        assert_eq!(
            entries[0].bytes_out,
            Some(expected),
            "post: длина тела запроса"
        );
        assert!(
            entries[0].bytes_out.unwrap() >= 2,
            "Content-Length >= len(body)"
        );
        assert_eq!(entries[1].bytes_out, None, "get: тела нет");
    }

    /// URL без хоста: типизированный `BadUrl`, одна audit-запись, в сеть не уходим.
    #[tokio::test]
    async fn bad_url_is_rejected_and_audited() {
        let (client, audit) = {
            let (policy, _) = policy_with_switch();
            guarded(policy)
        };
        let res = client
            .get("definitely not a url", EgressFeature::Probe)
            .await;
        assert!(matches!(res, Err(NetError::BadUrl)));
        assert_eq!(audit.len(), 1);
        assert!(!audit.entries()[0].allowed);
    }
}
