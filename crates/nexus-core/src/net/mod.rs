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

mod persist;
mod resolve;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use thiserror::Error;

pub use persist::{load as load_egress_state, save as save_egress_state, EgressState};
pub use resolve::{check_resolved_ips, Resolver, SystemResolver};

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
    /// Лента новостей (NF-4, W1): первый web-класс. По умолчанию ВЫКЛЮЧЕНА (consent при
    /// включении, W2/AC-NF-7); `allow_private=false` — приватные/LAN-хосты ей запрещены
    /// (W-аддендум: web-хосты только публичные, LAN-исключение E2/E6 не распространяется).
    NewsFeed,
    /// Web-агент чата (W1, срез 4): поиск через self-hosted SearXNG. Web-класс — `allow_private=false`,
    /// DNS-гард обязателен; consent = сохранённый URL SearXNG → авто-allowlist скоупа "web" (W2).
    /// По умолчанию ВЫКЛЮЧЕНА.
    Web,
}

impl EgressFeature {
    /// Индекс в таблице opt-in-флагов политики.
    fn idx(self) -> usize {
        match self {
            EgressFeature::Chat => 0,
            EgressFeature::Embed => 1,
            EgressFeature::Probe => 2,
            EgressFeature::NewsFeed => 3,
            EgressFeature::Web => 4,
        }
    }

    /// Web-класс (W-аддендум): приватные хосты запрещены даже из allowlist; DNS-гард обязателен.
    fn denies_private(self) -> bool {
        matches!(self, EgressFeature::NewsFeed | EgressFeature::Web)
    }
}

impl std::fmt::Display for EgressFeature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EgressFeature::Chat => "chat",
            EgressFeature::Embed => "embed",
            EgressFeature::Probe => "probe",
            EgressFeature::NewsFeed => "news_feed",
            EgressFeature::Web => "web",
        })
    }
}

impl std::str::FromStr for EgressFeature {
    type Err = ();
    /// Обратное к `Display` — для команды `set_egress_feature` (строка с фронта).
    /// `news_feed` НАМЕРЕННО не парсится: её тоггл — `news.json` (единственная истина consent,
    /// синхронизируется в политику через `set_news_config`/setup-хук), не egress-настройки.
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "chat" => Ok(EgressFeature::Chat),
            "embed" => Ok(EgressFeature::Embed),
            "probe" => Ok(EgressFeature::Probe),
            // "web" НАМЕРЕННО не парсится: consent = сохранённый URL SearXNG (websearch.json),
            // не egress-настройки — как news_feed.
            _ => Err(()),
        }
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
    /// включены по умолчанию; NewsFeed (web-класс) — ВЫКЛЮЧЕНА (consent, W2/AC-NF-7).
    features: [AtomicBool; 5],
    /// Exact-host allowlist ПО СКОУПАМ: "ai" — явные хосты `local.json ai.*` (E4, замещается на
    /// open-vault/смене настроек), "news" — хосты источников ленты (consent = включение фичи,
    /// NF-4). `check` смотрит объединение. `RwLock` — частые читатели, редкая замена.
    allowlist: RwLock<std::collections::HashMap<&'static str, HashSet<String>>>,
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
                AtomicBool::new(false), // NewsFeed: web-класс не из коробки (W2)
                AtomicBool::new(false), // Web: web-агент не из коробки (W2, consent = URL SearXNG)
            ],
            allowlist: RwLock::new(std::collections::HashMap::new()),
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
        // 4а. Web-класс (NewsFeed): приватные/LAN-хосты запрещены ДАЖЕ из allowlist
        //     (W-аддендум `allow_private=false`; AC-NF-8 — литеральные IP; домены добивает
        //     DNS-гард фетчера). Metadata уже отрезан шагом 1.
        if feature.denies_private() && is_private_host(host) {
            return Err(EgressDenied::HostNotAllowed(Redacted::new(
                host.to_string(),
            )));
        }
        // 4б. Хост: приватный/loopback (local-first, AC-EGR-9; не для web-класса) ИЛИ
        //     явный allowlist любого скоупа (E4 — "ai"; NF-4 — "news").
        let allowed = (!feature.denies_private() && is_private_host(host))
            || self
                .allowlist
                .read()
                .map(|m| m.values().any(|set| set.contains(host)))
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

    /// Текущее состояние фичи — для UI настроек/персиста (срез 2).
    pub fn is_feature_enabled(&self, feature: EgressFeature) -> bool {
        self.features[feature.idx()].load(Ordering::Relaxed)
    }

    /// Заменяет allowlist скоупа "ai" (E4: пересобирается из `local.json ai.*` на open-vault и
    /// при смене настроек). Consent на pull-changed URL — хвост среза 2 (персист E5).
    pub fn set_allowlist(&self, hosts: impl IntoIterator<Item = String>) {
        self.set_scoped_allowlist("ai", hosts);
    }

    /// Заменяет allowlist именованного скоупа ("news" — хосты источников ленты, NF-4):
    /// скоупы независимы, `check` смотрит объединение.
    pub fn set_scoped_allowlist(
        &self,
        scope: &'static str,
        hosts: impl IntoIterator<Item = String>,
    ) {
        if let Ok(mut m) = self.allowlist.write() {
            m.insert(scope, hosts.into_iter().collect());
        }
    }
}

/// Контекст прогона для корреляции эгресса (ADR D7-b: **ЯВНО ПРОБРАСЫВАЕТСЯ**, не task-local/процесс-
/// глобальный слот). Несёт AgentRun `run_id`, который [`EgressAudit::record`] пишет в audit-строку.
/// `Copy` — дёшево передаётся по значению на каждый guarded-вызов (как `cancel` ездит по per-call
/// каналу). Конструкторы ТОЛЬКО [`RunCtx::NONE`] (нет прогона: chat/embed/probe вне агента) и
/// [`RunCtx::run`] (живой прогон) — `Default` НЕ выводим намеренно, чтобы выбор «без run-context» был
/// всегда ЯВНЫМ и grep-аудируемым (исключаем `..Default::default()`, молча дающий uncorrelated-строку
/// внутри прогона). Per-call значение УСТРАНЯЕТ кросс-атрибуцию конкурентных прогонов (каждый несёт
/// свой run_id в СВОЁМ стеке вызова, общего изменяемого состояния нет).
#[derive(Debug, Clone, Copy)]
pub struct RunCtx {
    pub run_id: Option<i64>,
}

impl RunCtx {
    /// Нет прогона: эгресс не коррелирован (chat/embed/probe вне агента). `run_id=None` в audit.
    pub const NONE: RunCtx = RunCtx { run_id: None };
    /// Живой прогон агента: эгресс коррелируется на этот `run_id` в audit.
    pub fn run(run_id: i64) -> Self {
        Self {
            run_id: Some(run_id),
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
    /// AgentRun correlation-id (AGENT-3a): связывает эгресс с прогоном агента. Берётся из
    /// ЯВНО ПРОБРОШЕННОГО [`RunCtx`] per-call (не из процесс-глобального слота): `Some(run_id)` для
    /// эгресса внутри прогона, `None` для chat/embed/probe вне агента ([`RunCtx::NONE`]).
    pub run_id: Option<i64>,
}

/// Неотключаемый append-only журнал эгресса ядра (инвариант — как брокерский `AuditLog`):
/// `record()` приватен для `net/`, публичны только чтения; чистить нельзя by design (AC-EGR-4).
///
/// ДВА слоя (P0-b):
/// 1. **In-memory** `Mutex<Vec<..>>` — снимки `entries()`, pre-vault эгресс (БД ещё не открыта) и тесты.
/// 2. **Durable** опциональный [`WriteActor`]-сток, выставляемый ПОСЛЕ конструирования через
///    [`set_writer`](Self::set_writer) (журнал строится на старте приложения ДО открытия vault-БД).
///    Когда сток есть, `record()` персистит запись append-only в `egress_audit` **ПЕРЕД возвратом**
///    (write-before-act: `authorize` awaits `record()` до сокета). Pre-vault окно: сток ещё None →
///    запись живёт только в памяти (durable-история начинается с момента `set_writer`).
#[derive(Debug, Default)]
pub struct EgressAudit {
    /// `Mutex` — записи короткие, журнал делится между провайдерами через `Arc`.
    entries: Mutex<Vec<EgressAuditEntry>>,
    /// Durable-сток: `Some(WriteActor)` после `set_writer` (открытие vault). До него — pre-vault окно
    /// (только in-memory). `Mutex` (а не `OnceLock`): десктоп может переоткрыть vault (новая БД) →
    /// сток заменяем. `WriteActor` клонируется дёшево (общий канал).
    writer: Mutex<Option<crate::db::WriteActor>>,
}

impl EgressAudit {
    /// Подключает durable-сток ПОСЛЕ конструирования (журнал строится на старте ДО открытия vault-БД).
    /// Зовётся из composition-root: десктоп — в `open_vault` после `Database::open`; agentd — в main
    /// после открытия БД. С этого момента `record()` персистит каждый эгресс в `egress_audit`.
    pub fn set_writer(&self, writer: crate::db::WriteActor) {
        if let Ok(mut w) = self.writer.lock() {
            *w = Some(writer);
        }
    }

    /// Единственная точка записи — зовётся ТОЛЬКО из [`GuardedClient`] (приватность = append-only).
    ///
    /// **Write-before-act** (P0-b): (а) пушит запись в in-memory Vec, затем (б) если durable-сток есть —
    /// персистит её append-only в `egress_audit` И ЖДЁТ коммита ПЕРЕД возвратом. `authorize` awaits
    /// этот `record()` до отправки сокета → durable-строка существует ДО любого сетевого действия.
    /// Сбой записи в БД НЕ роняет эгресс (best-effort durable; in-memory-слой и so хранит запись),
    /// но логируется: durable-история — подотчётность, не gate на сам запрос.
    async fn record(
        &self,
        feature: EgressFeature,
        host: String,
        bytes_out: Option<usize>,
        decision: &Result<(), EgressDenied>,
        ctx: RunCtx,
    ) {
        // run_id берётся из ЯВНО ПРОБРОШЕННОГО per-call контекста (AGENT-3a) — НЕ из процесс-глобального
        // слота: конкурентные прогоны не перетирают атрибуцию друг друга (каждый несёт свой ctx в стеке).
        let run_id = ctx.run_id;
        let entry = EgressAuditEntry {
            feature,
            // Хост хранится РЕАЛЬНЫЙ: локальная БД vault — собственный аудит пользователя; Redacted —
            // про утечку в Debug/логи, не про хранение на своём диске (см. 020_egress_audit.sql).
            host: Redacted::new(host),
            bytes_out,
            allowed: decision.is_ok(),
            denied_reason: decision.as_ref().err().map(|d| d.to_string()),
            run_id,
        };

        // (а) In-memory — всегда (снимки/pre-vault/тесты).
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(entry.clone());
        }

        // (б) Durable — если сток подключён. Клонируем WriteActor под мьютексом и сразу отпускаем
        //     лок (await под std::Mutex недопустим). Ждём коммит ПЕРЕД возвратом (write-before-act).
        let writer = self.writer.lock().ok().and_then(|w| w.clone());
        if let Some(writer) = writer {
            let feature_str = entry.feature.to_string();
            let host = entry.host.expose().clone();
            let bytes_out = entry.bytes_out.map(|b| b as i64);
            let allowed = entry.allowed;
            let denied_reason = entry.denied_reason.clone();
            let run_id = entry.run_id;
            let created_at = crate::scheduler::now_secs();
            let res = writer
                .call(move |conn| {
                    conn.execute(
                        "INSERT INTO egress_audit \
                         (feature, host, bytes_out, allowed, denied_reason, run_id, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            feature_str,
                            host,
                            bytes_out,
                            allowed as i64,
                            denied_reason,
                            run_id,
                            created_at,
                        ],
                    )
                    .map(|_| ())
                })
                .await;
            if let Err(e) = res {
                // Best-effort: durable-сбой не роняет эгресс (in-memory-слой запись сохранил), но
                // логируем без хоста (Redacted-инвариант: в логи реальный хост не утекает).
                tracing::warn!(error = %e, "egress-audit: durable-запись не удалась (in-memory сохранён)");
            }
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

/// Тип фабрики тюнинга билдера: вызывается на КАЖДЫЙ запрос (нужно пересобрать клиент с пином
/// проверенного IP — `resolve_to_addrs` — чтобы DNS не «перепрыгнул» между check и connect, TOCTOU).
/// Поэтому `Fn` (а не `FnOnce`) + `Send + Sync` (клиент клонируется между задачами) под `Arc`.
type TuneFn = Arc<dyn Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync>;

/// Guarded HTTP-клиент ядра — ЕДИНСТВЕННАЯ дверь исходящего HTTP (E1, AC-EGR-1). Каждый запрос:
/// `policy.check` (host-string-гейт) → **резолв + проверка ВСЕХ IP** (DNS-rebinding/SSRF-гард, P0-a)
/// → запись в audit (успех И отказ) → реальный I/O **с пином проверенного IP**.
/// Клонирование дёшево (`reqwest::Client`/`Arc` внутри).
#[derive(Clone)]
pub struct GuardedClient {
    /// Фабрика тюнинга — пересобирает клиент с `resolve_to_addrs` (пин проверенного IP) на КАЖДЫЙ
    /// запрос; держит таймауты/UA вызывающего и приватный `core_client_builder` (redirect=none).
    tune: TuneFn,
    policy: Arc<EgressPolicy>,
    audit: Arc<EgressAudit>,
    /// DNS-резолвер для гарда (боевой [`SystemResolver`]; в тестах/web-классе подменяется).
    resolver: Arc<dyn Resolver>,
}

impl GuardedClient {
    /// Строит guarded-клиент поверх приватного `core_client_builder` (redirect=none сохраняется,
    /// AC-EGR-7); `tune` добавляет таймауты вызывающего, политику редиректов не трогать.
    /// `tune` зовётся на каждый запрос (пересборка с пином IP) — потому `Fn + Send + Sync`.
    pub fn new(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        tune: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync + 'static,
    ) -> Result<Self, NetError> {
        let tune: TuneFn = Arc::new(tune);
        // Build-and-discard: валидируем конфиг билдера СРАЗУ (как раньше), чтобы `new` отдал `Err`
        // на битом тюнинге, а не падал на первом запросе. Сам клиент per-request пересобирается.
        let _ = tune(core_client_builder()).build()?;
        Ok(Self {
            tune,
            policy,
            audit,
            resolver: Arc::new(SystemResolver),
        })
    }

    /// Подменяет резолвер: web-класс (news/websearch) инъектит свой `Arc<dyn Resolver>` (в проде —
    /// [`SystemResolver`], в их тестах — мок), а гард резолв→проверка→пин делает САМ core-путь
    /// (P0-a, единый источник истины). Боевой core-путь chat/embed/probe использует
    /// [`SystemResolver`] по умолчанию (без вызова этого сеттера).
    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.resolver = resolver;
        self
    }

    /// Профиль chat-стрима: общего таймаута нет (стрим долгий, first-token/idle-таймауты — у
    /// провайдера), `connect_timeout` страхует от зависшего коннекта. INFER-CFG: длительность
    /// принимается параметром (из `ChatConfig::connect_timeout()`, дефолт 30 с — безопаснее для
    /// cold-start V100, ок на LAN) — раньше был хардкод 15 с.
    pub fn for_chat(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        connect_timeout: Duration,
    ) -> Result<Self, NetError> {
        Self::new(policy, audit, move |b| b.connect_timeout(connect_timeout))
    }

    /// Профиль эмбеддинга: общий таймаут (батчи бывают тяжёлые). INFER-CFG: длительность принимается
    /// параметром (из `EmbeddingConfig::timeout()`, дефолт 60 с) — раньше был хардкод 60 с.
    pub fn for_embedding(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        timeout: Duration,
    ) -> Result<Self, NetError> {
        Self::new(policy, audit, move |b| b.timeout(timeout))
    }

    /// Профиль probe (проба размерности / «Проверить связь»): короткий таймаут вызывающего.
    pub fn for_probe(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        timeout: Duration,
    ) -> Result<Self, NetError> {
        Self::new(policy, audit, move |b| b.timeout(timeout))
    }

    /// GET через политику (probe `/v1/models`). `bytes_out=None` — тела запроса нет (AC-EGR-10).
    /// `ctx` — per-call run-контекст (AGENT-3a): [`RunCtx::NONE`] вне прогона, [`RunCtx::run`] внутри.
    pub async fn get(
        &self,
        url: &str,
        feature: EgressFeature,
        ctx: RunCtx,
    ) -> Result<reqwest::Response, NetError> {
        let client = self.authorize(url, feature, None, ctx).await?;
        Ok(client.get(url).send().await?)
    }

    /// POST JSON-тела через политику. `bytes_out=Some(len)` — длина сериализованного тела ЗАПРОСА
    /// известна и для стрим-ответа (AC-EGR-10: best-effort, тело запроса, не ответ).
    /// `ctx` — per-call run-контекст (AGENT-3a): [`RunCtx::NONE`] вне прогона, [`RunCtx::run`] внутри.
    pub async fn post_json(
        &self,
        url: &str,
        feature: EgressFeature,
        body: &serde_json::Value,
        ctx: RunCtx,
    ) -> Result<reqwest::Response, NetError> {
        let bytes = serde_json::to_vec(body).expect("serde_json::Value сериализуем всегда");
        let client = self.authorize(url, feature, Some(bytes.len()), ctx).await?;
        Ok(client
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

    /// Авторизация per-request (порядок — дизайн-инвариант): host-string-гейт политики → **резолв +
    /// проверка ВСЕХ IP** (DNS-rebinding/SSRF, P0-a) → ровно ОДНА запись audit (успех И отказ,
    /// AC-EGR-4). Возвращает клиент с **пином** проверенного IP (`resolve_to_addrs`): коннект пойдёт
    /// на проверенный адрес, а не на повторный резолв атакующего DNS (TOCTOU). Любой отказ — ДО сокета.
    async fn authorize(
        &self,
        url: &str,
        feature: EgressFeature,
        bytes_out: Option<usize>,
        ctx: RunCtx,
    ) -> Result<reqwest::Client, NetError> {
        let parsed = reqwest::Url::parse(url).ok();
        let host = parsed
            .as_ref()
            .and_then(|u| u.host_str().map(str::to_string));
        let Some(host) = host else {
            // URL без хоста: аудитим сырой url (redacted) с отказом и не уходим в сеть.
            self.audit
                .record(
                    feature,
                    url.to_string(),
                    bytes_out,
                    &Err(EgressDenied::HostNotAllowed(Redacted::new(url.to_string()))),
                    ctx,
                )
                .await;
            return Err(NetError::BadUrl);
        };

        // 1. Host-string-гейт (metadata/офлайн/opt-in/allowlist|приватность) — без сети.
        if let Err(denied) = self.policy.check(&host, feature) {
            self.audit
                .record(feature, host, bytes_out, &Err(denied.clone()), ctx)
                .await;
            return Err(NetError::from(denied));
        }

        // 2. DNS-rebinding/SSRF-гард (P0-a): резолв → проверка ВСЕХ IP. host-литерал-IP резолвится
        //    сам в себя — гард всё равно отрабатывает (defense-in-depth поверх host-гейта).
        let ips = match self.resolver.resolve(&host).await {
            Ok(ips) => ips,
            Err(_) => {
                // Резолв упал — НЕ молчаливый allow: типизированный отказ + audit как denial.
                let denied = EgressDenied::HostNotAllowed(Redacted::new(host.clone()));
                self.audit
                    .record(feature, host, bytes_out, &Err(denied.clone()), ctx)
                    .await;
                return Err(NetError::from(denied));
            }
        };
        let ip_decision = check_resolved_ips(&ips, feature.denies_private());
        if let Err(denied) = ip_decision {
            self.audit
                .record(feature, host, bytes_out, &Err(denied.clone()), ctx)
                .await;
            return Err(NetError::from(denied));
        }

        // 3. Успех — ровно одна audit-запись.
        self.audit
            .record(feature, host.clone(), bytes_out, &Ok(()), ctx)
            .await;

        // 4. ПИН: пересобираем клиент с `resolve_to_addrs` на первый проверенный IP (порт из URL).
        //    Анти-TOCTOU: коннект гарантированно идёт на проверенный адрес. redirect=none сохранён
        //    (через `core_client_builder` в фабрике `tune`).
        let port = parsed
            .as_ref()
            .and_then(|u| u.port_or_known_default())
            .unwrap_or(443);
        let pinned = std::net::SocketAddr::new(ips[0], port);
        let client = (self.tune)(core_client_builder())
            .resolve_to_addrs(&host, &[pinned])
            .build()?;
        Ok(client)
    }

    /// Тест-фикстура: политика с дефолтами (фичи включены, офлайн выключен, allowlist пуст) —
    /// мок-серверы на loopback проходят как `is_private_host` без живого allowlist.
    /// Под `#[cfg(any(test, feature = "test-util"))]` — доступна dev-тестам потребителя (CORE-1).
    #[cfg(any(test, feature = "test-util"))]
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

    /// Гард с боевым `SystemResolver` (литералы 127.0.0.1/IP резолвятся в себя без сети).
    fn guarded(policy: Arc<EgressPolicy>) -> (GuardedClient, Arc<EgressAudit>) {
        let audit = Arc::new(EgressAudit::default());
        let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();
        (client, audit)
    }

    /// Гард с МОК-резолвером: любой хост → заданный список IP (DNS-rebinding-сценарии без сети).
    fn guarded_with_ips(
        policy: Arc<EgressPolicy>,
        ips: Vec<std::net::IpAddr>,
    ) -> (GuardedClient, Arc<EgressAudit>) {
        let audit = Arc::new(EgressAudit::default());
        let resolver = Arc::new(resolve::test_support::FixedResolver::new(ips));
        let client = GuardedClient::new(policy, audit.clone(), |b| b)
            .unwrap()
            .with_resolver(resolver);
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
            .get(
                &format!("http://{addr}/"),
                EgressFeature::Probe,
                RunCtx::NONE,
            )
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
                RunCtx::NONE,
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

    /// NF-4 (AC-NF-7/8): NewsFeed — web-класс. Выключена из коробки (consent W2); после
    /// включения публичный хост из "news"-скоупа проходит, а приватный/LAN запрещён ДАЖЕ из
    /// allowlist (`allow_private=false`, W-аддендум); скоупы "ai"/"news" независимы; local-first
    /// для Chat не задет.
    #[test]
    fn news_feed_is_web_class_private_denied_even_allowlisted() {
        let (policy, _) = policy_with_switch();
        assert!(
            matches!(
                policy.check("feeds.example.com", EgressFeature::NewsFeed),
                Err(EgressDenied::FeatureNotEnabled(EgressFeature::NewsFeed))
            ),
            "web-класс не из коробки"
        );
        policy.set_feature_enabled(EgressFeature::NewsFeed, true);
        assert!(matches!(
            policy.check("feeds.example.com", EgressFeature::NewsFeed),
            Err(EgressDenied::HostNotAllowed(_))
        ));
        policy.set_scoped_allowlist(
            "news",
            ["feeds.example.com".to_string(), "192.168.0.5".to_string()],
        );
        assert_eq!(
            policy.check("feeds.example.com", EgressFeature::NewsFeed),
            Ok(())
        );
        assert!(
            matches!(
                policy.check("192.168.0.5", EgressFeature::NewsFeed),
                Err(EgressDenied::HostNotAllowed(_))
            ),
            "allow_private=false: приватный запрещён даже из allowlist"
        );
        // Скоупы независимы: ai-замещение не трогает news.
        policy.set_allowlist(["api.other.com".to_string()]);
        assert_eq!(
            policy.check("feeds.example.com", EgressFeature::NewsFeed),
            Ok(()),
            "news-скоуп пережил замену ai-скоупа"
        );
        // Local-first для Chat не задет web-правилом.
        assert_eq!(policy.check("192.168.0.5", EgressFeature::Chat), Ok(()));
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
                RunCtx::NONE,
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
                RunCtx::NONE,
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
            .get(
                &format!("http://{addr}/v1/models"),
                EgressFeature::Probe,
                RunCtx::NONE,
            )
            .await
            .expect("loopback живёт при офлайн (E2)");
        assert_eq!(resp.status().as_u16(), 200);
        server.join().unwrap();

        let denied = client
            .get("http://203.0.113.7/", EgressFeature::Probe, RunCtx::NONE)
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
            .get(
                &format!("http://{addr}/v1/models"),
                EgressFeature::Probe,
                RunCtx::NONE,
            )
            .await
            .expect("loopback разрешён");
        server.join().unwrap();
        let denied = client
            .get(
                "http://api.example.com/v1/models",
                EgressFeature::Probe,
                RunCtx::NONE,
            )
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
                RunCtx::NONE,
            )
            .await
            .expect("loopback разрешён");
        server.join().unwrap();
        let denied_get = client
            .get(
                "http://api.example.com/x",
                EgressFeature::Probe,
                RunCtx::NONE,
            )
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

    /// P0-a (DNS-rebinding на CORE-пути): chat-хост, резолвящийся в metadata 169.254.169.254,
    /// отклоняется ДО коннекта — типизированным отказом (не сетевой ошибкой) и аудитится как denial.
    /// Мок-listener обязан НЕ принять соединение.
    #[tokio::test]
    async fn chat_host_resolving_to_metadata_is_denied_before_connect() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();

        let (policy, _) = policy_with_switch();
        policy.set_allowlist(["chat.example.com".to_string()]); // host-string-гейт пропустит
        let (client, audit) = guarded_with_ips(policy, vec!["169.254.169.254".parse().unwrap()]);

        let res = client
            .post_json(
                "http://chat.example.com/v1/chat/completions",
                EgressFeature::Chat,
                &serde_json::json!({"messages": []}),
                RunCtx::NONE,
            )
            .await;
        assert!(
            matches!(res, Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))),
            "rebind на metadata режется типизированно: {res:?}"
        );
        assert!(
            matches!(
                listener.accept(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
            ),
            "0 коннектов: DNS-гард отрезал ДО сокета (P0-a)"
        );
        let entries = audit.entries();
        assert_eq!(entries.len(), 1, "ровно одна audit-запись на отказ");
        assert!(!entries[0].allowed, "rebind аудитится как denial");
    }

    /// P0-a (local-first сохранён): chat-хост, резолвящийся в loopback/LAN, ДОПУСКАЕТСЯ — приватные
    /// IP для chat живут (LAN-LLM). Реальный коннект на loopback-мок проходит (пин не ломает loopback).
    #[tokio::test]
    async fn chat_host_resolving_to_loopback_or_lan_is_allowed() {
        // Реальный loopback-сервер; мок-резолвер отдаёт его адрес как «резолв» публичного имени.
        let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let (policy, _) = policy_with_switch();
        policy.set_allowlist(["chat.example.com".to_string()]);
        let (client, audit) = guarded_with_ips(policy, vec![addr.ip()]);

        // URL-порт должен совпасть с портом пина → используем порт мок-сервера в URL.
        let url = format!("http://chat.example.com:{}/v1/models", addr.port());
        let resp = client
            .get(&url, EgressFeature::Chat, RunCtx::NONE)
            .await
            .expect("loopback/LAN для chat живёт (local-first)");
        assert_eq!(resp.status().as_u16(), 200);
        server.join().unwrap();
        assert!(audit.entries()[0].allowed, "успех аудитится как allowed");

        // И чисто политически: LAN-IP (192.168.x) для chat проходит ip-гард.
        let (policy2, _) = policy_with_switch();
        policy2.set_allowlist(["lan.example.com".to_string()]);
        let (client2, _) = guarded_with_ips(policy2, vec!["192.168.0.31".parse().unwrap()]);
        // Коннекта к 192.168.0.31 не будет (нет сервера) — но гард обязан ПРОПУСТИТЬ (ip-allow),
        // отказ может прийти только сетевой (Http), не Denied. Проверяем именно это разграничение.
        let res = client2
            .get(
                "http://lan.example.com/v1/models",
                EgressFeature::Chat,
                RunCtx::NONE,
            )
            .await;
        assert!(
            !matches!(res, Err(NetError::Denied(_))),
            "LAN для chat НЕ отклоняется политикой/гардом (local-first): {res:?}"
        );
    }

    /// P0-a (web-класс): NewsFeed-хост, резолвящийся в приватный LAN-IP, отклоняется (deny_private).
    #[tokio::test]
    async fn web_class_host_resolving_to_private_is_denied() {
        let (policy, _) = policy_with_switch();
        policy.set_feature_enabled(EgressFeature::NewsFeed, true);
        policy.set_scoped_allowlist("news", ["feeds.example.com".to_string()]);
        let (client, audit) = guarded_with_ips(policy, vec!["10.0.0.7".parse().unwrap()]);

        let res = client
            .get(
                "https://feeds.example.com/rss",
                EgressFeature::NewsFeed,
                RunCtx::NONE,
            )
            .await;
        assert!(
            matches!(res, Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))),
            "web-класс: приватный резолв denied: {res:?}"
        );
        assert!(!audit.entries()[0].allowed);
    }

    /// P0-a: пустой резолв → типизированный отказ (нечего пинить), аудит как denial, без сети.
    #[tokio::test]
    async fn empty_resolution_is_denied() {
        let (policy, _) = policy_with_switch();
        policy.set_allowlist(["chat.example.com".to_string()]);
        let (client, audit) = guarded_with_ips(policy, vec![]);
        let res = client
            .get(
                "http://chat.example.com/v1/models",
                EgressFeature::Chat,
                RunCtx::NONE,
            )
            .await;
        assert!(matches!(
            res,
            Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))
        ));
        assert_eq!(audit.len(), 1);
        assert!(!audit.entries()[0].allowed);
    }

    /// URL без хоста: типизированный `BadUrl`, одна audit-запись, в сеть не уходим.
    #[tokio::test]
    async fn bad_url_is_rejected_and_audited() {
        let (client, audit) = {
            let (policy, _) = policy_with_switch();
            guarded(policy)
        };
        let res = client
            .get("definitely not a url", EgressFeature::Probe, RunCtx::NONE)
            .await;
        assert!(matches!(res, Err(NetError::BadUrl)));
        assert_eq!(audit.len(), 1);
        assert!(!audit.entries()[0].allowed);
    }

    /// Открывает временную vault-БД (миграции применены, в т.ч. 020 egress_audit). `(Database, TempDir)`
    /// в таком порядке: при выходе из scope сначала закрывается БД, потом удаляется каталог.
    async fn temp_db() -> (crate::db::Database, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = crate::db::Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .expect("open db");
        (db, dir)
    }

    /// Снимок durable-журнала: `(feature, host, allowed, denied_is_some, run_id)` в порядке вставки.
    async fn durable_rows(
        db: &crate::db::Database,
    ) -> Vec<(String, String, bool, bool, Option<i64>)> {
        db.reader()
            .query(|c| {
                let mut stmt = c.prepare(
                    "SELECT feature, host, allowed, denied_reason, run_id \
                     FROM egress_audit ORDER BY id",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)? != 0,
                            r.get::<_, Option<String>>(3)?.is_some(),
                            r.get::<_, Option<i64>>(4)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .unwrap()
    }

    /// P0-b (durable persist): с подключённым writer `record()` персистит строку в `egress_audit` —
    /// реальный host, decision, run_id=None (scaffold). Успех И отказ оба durable.
    #[tokio::test]
    async fn durable_record_persists_row_with_writer() {
        let (db, _dir) = temp_db().await;
        let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");

        let audit = Arc::new(EgressAudit::default());
        audit.set_writer(db.writer().clone());
        let (policy, _) = policy_with_switch();
        policy.set_allowlist(["api.example.com".to_string()]);
        let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();

        // Успех на loopback.
        client
            .get(
                &format!("http://{addr}/v1/models"),
                EgressFeature::Probe,
                RunCtx::NONE,
            )
            .await
            .expect("loopback разрешён");
        server.join().unwrap();
        // Отказ: публичный хост вне allowlist (api.example.com в allowlist → используем другой).
        let denied = client
            .get(
                "http://blocked.example.com/x",
                EgressFeature::Probe,
                RunCtx::NONE,
            )
            .await;
        assert!(matches!(denied, Err(NetError::Denied(_))));

        let rows = durable_rows(&db).await;
        assert_eq!(rows.len(), 2, "оба эгресса durable-персистнуты");
        // Строка 1 — успех на реальном loopback-хосте (НЕ Redacted в БД).
        assert_eq!(rows[0].0, "probe");
        assert_eq!(
            rows[0].1, "127.0.0.1",
            "host хранится РЕАЛЬНЫЙ, не Redacted"
        );
        assert!(
            rows[0].2 && !rows[0].3,
            "успех: allowed=1, denied_reason=NULL"
        );
        assert_eq!(rows[0].4, None, "run_id scaffold: None");
        // Строка 2 — отказ.
        assert_eq!(rows[1].1, "blocked.example.com");
        assert!(
            !rows[1].2 && rows[1].3,
            "отказ: allowed=0, denied_reason set"
        );
    }

    /// P0-b (write-before-act ORDERING): durable denial-строка существует ДО возврата `authorize` —
    /// т.е. ДО того, как мог бы уйти сокет. Гард-denied запрос (DNS-гард режет до коннекта) оставляет
    /// durable-строку, причём listener-мок НЕ принимает соединение (0 коннектов). Так как `authorize`
    /// awaits `record()` синхронно перед I/O, наличие строки сразу после await доказывает порядок.
    #[tokio::test]
    async fn durable_denial_row_exists_before_authorize_returns() {
        let (db, _dir) = temp_db().await;
        // Listener, который НЕ должен принять соединение (отказ — до сокета).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();

        let audit = Arc::new(EgressAudit::default());
        audit.set_writer(db.writer().clone());
        let (policy, _) = policy_with_switch();
        policy.set_allowlist(["chat.example.com".to_string()]); // host-гейт пропустит
                                                                // DNS-гард: хост резолвится в metadata → denied ДО коннекта (P0-a).
        let resolver = Arc::new(resolve::test_support::FixedResolver::new(vec![
            "169.254.169.254".parse().unwrap(),
        ]));
        let client = GuardedClient::new(policy, audit.clone(), |b| b)
            .unwrap()
            .with_resolver(resolver);

        let res = client
            .post_json(
                "http://chat.example.com/v1/chat/completions",
                EgressFeature::Chat,
                &serde_json::json!({"messages": []}),
                RunCtx::NONE,
            )
            .await;
        assert!(
            matches!(res, Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))),
            "rebind на metadata режется типизированно: {res:?}"
        );
        // 0 коннектов: гард отрезал ДО сокета.
        assert!(
            matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "denied-запрос не должен был коснуться сокета (write-before-act)"
        );
        // Durable-строка УЖЕ есть сразу после возврата authorize (record awaited перед любым I/O):
        // строка существует, а сокета не было → запись произошла ДО (несуществующей) отправки.
        let rows = durable_rows(&db).await;
        assert_eq!(rows.len(), 1, "denial durable-персистнут write-before-act");
        assert_eq!(rows[0].0, "chat");
        assert!(!rows[0].2, "denial: allowed=0");
        assert!(rows[0].3, "denial_reason set");
    }

    /// P0-b (pre-vault окно / тесты): БЕЗ writer'а `record()` всё равно работает — пишет только in-memory.
    /// Это сценарий pre-vault эгресса (БД ещё не открыта) и всех тестов с `EgressAudit::default()`.
    #[tokio::test]
    async fn record_without_writer_is_in_memory_only() {
        let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let (client, audit) = guarded(policy_with_switch().0);
        client
            .get(
                &format!("http://{addr}/v1/models"),
                EgressFeature::Probe,
                RunCtx::NONE,
            )
            .await
            .expect("loopback разрешён");
        server.join().unwrap();
        assert_eq!(audit.len(), 1, "in-memory работает без writer (pre-vault)");
        assert!(audit.entries()[0].allowed);
    }

    /// P0-b (write-before-act ORDERING, SUCCESS-путь): durable success-строка (`allowed=1`) существует
    /// ДО возврата `authorize` — т.е. ДО любого сокета/send. Зовём приватный `authorize` напрямую (НЕ
    /// `get`), поэтому МЕЖДУ awaited `record()` и проверкой БД сетевого I/O нет вообще: наличие строки
    /// сразу после `authorize().await` доказывает, что success-`record()` закоммичен ПЕРЕД отправкой.
    /// Регрессия, делающая success-`record()` fire-and-forget (не awaited внутри authorize), валит тест.
    /// Listener — принимающий loopback (mirror denial-теста), но на коннект мы НЕ полагаемся: `authorize`
    /// возвращает только пин-клиент, send не делается, так что сокет остаётся нетронутым.
    #[tokio::test]
    async fn durable_success_row_exists_before_authorize_returns() {
        let (db, _dir) = temp_db().await;
        // Принимающий loopback-listener (как в success-кейсе durable_record_persists_*). На приём
        // соединения тест НЕ полагается — он лишь даёт реальный адрес, резолвящийся в 127.0.0.1.
        let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");

        let audit = Arc::new(EgressAudit::default());
        audit.set_writer(db.writer().clone());
        let (policy, _) = policy_with_switch();
        let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();

        // Вызываем ПРИВАТНЫЙ authorize напрямую: проходит host-гейт (loopback local-first) + DNS/SSRF-гард
        // (127.0.0.1 резолвится в себя), затем success-`record()` AWAITED, затем строится пин-клиент.
        // send НЕ делается → между коммитом записи и проверкой БД сетевого I/O нет.
        let url = format!("http://{addr}/v1/models");
        let authorized = client
            .authorize(&url, EgressFeature::Probe, None, RunCtx::NONE)
            .await;
        assert!(authorized.is_ok(), "loopback success-путь: {authorized:?}");

        // Durable success-строка УЖЕ есть сразу после возврата authorize, ДО какого-либо send.
        // Будь success-`record()` fire-and-forget — строки тут могло не быть (тест бы упал).
        let rows = durable_rows(&db).await;
        assert_eq!(
            rows.len(),
            1,
            "success durable-персистнут write-before-act (ДО send)"
        );
        assert_eq!(rows[0].0, "probe");
        assert_eq!(rows[0].1, "127.0.0.1", "host хранится РЕАЛЬНЫЙ");
        assert!(
            rows[0].2 && !rows[0].3,
            "success: allowed=1, denied_reason=NULL"
        );

        // listener так и не принял соединение (send не вызывался) — закрываем его, дренируя поток.
        drop(client);
        drop(server);
    }

    /// P0-b (vault re-open writer swap): `record()` перечитывает writer ПЕР-вызов (под мьютексом), поэтому
    /// после `set_writer(B)` записи идут ТОЛЬКО в B, а старая БД A остаётся со своей единственной строкой.
    /// Доказывает атомарность подмены стока на переоткрытии vault и отсутствие stale-writer-в-старую-БД.
    /// Эгресс — denied (host-гейт режет публичный хост вне allowlist) для простоты: durable-строка пишется
    /// и на отказе (write-before-act), сеть не нужна.
    #[tokio::test]
    async fn writer_swap_on_vault_reopen_routes_to_new_db_only() {
        let (db_a, _dir_a) = temp_db().await;
        let (db_b, _dir_b) = temp_db().await;

        let audit = Arc::new(EgressAudit::default());
        let (policy, _) = policy_with_switch();
        let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();

        // Сток = A. Один denied-эгресс → строка в A.
        audit.set_writer(db_a.writer().clone());
        let denied_a = client
            .get(
                "http://first.example.com/x",
                EgressFeature::Probe,
                RunCtx::NONE,
            )
            .await;
        assert!(matches!(denied_a, Err(NetError::Denied(_))));

        let rows_a1 = durable_rows(&db_a).await;
        assert_eq!(rows_a1.len(), 1, "1-я строка в A");
        assert_eq!(rows_a1[0].1, "first.example.com");
        assert!(durable_rows(&db_b).await.is_empty(), "B ещё пуста");

        // Переоткрытие vault: подменяем сток на B. Следующий эгресс должен попасть ТОЛЬКО в B.
        audit.set_writer(db_b.writer().clone());
        let denied_b = client
            .get(
                "http://second.example.com/y",
                EgressFeature::Probe,
                RunCtx::NONE,
            )
            .await;
        assert!(matches!(denied_b, Err(NetError::Denied(_))));

        // B содержит ровно 2-ю строку; A — по-прежнему только 1-ю (stale-writer в A не писал).
        let rows_b = durable_rows(&db_b).await;
        assert_eq!(rows_b.len(), 1, "2-я строка ТОЛЬКО в B");
        assert_eq!(rows_b[0].1, "second.example.com");
        let rows_a2 = durable_rows(&db_a).await;
        assert_eq!(rows_a2.len(), 1, "A не изменилась после подмены стока");
        assert_eq!(
            rows_a2[0].1, "first.example.com",
            "A хранит свою 1-ю строку"
        );
    }
}
