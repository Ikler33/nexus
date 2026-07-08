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
    /// **WEB-FETCH-PUBLIC (owner-gated 2026-06-22):** когда `true`, фича `Web` (агентский `web.fetch`)
    /// допускает ЛЮБОЙ ПУБЛИЧНЫЙ хост без allowlist — для deep-research. ВСЕ остальные рубежи сохранены:
    /// metadata (шаг 1), офлайн-kill (шаг 2), opt-in (шаг 3), `deny_private` (шаг 4а — приватные/LAN
    /// режутся) + DNS-rebind/SSRF-гард + redirect=none + audit в `authorize`. Default `false`
    /// (allowlist-only). КАСАЕТСЯ ТОЛЬКО `Web` (НЕ `NewsFeed` — у неё consent по конкретным URL).
    web_allow_public: AtomicBool,
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
            web_allow_public: AtomicBool::new(false),
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
        // 4б. Хост: приватный/loopback (local-first, AC-EGR-9; не для web-класса) ИЛИ явный allowlist
        //     любого скоупа (E4 — "ai"; NF-4 — "news") ИЛИ — WEB-FETCH-PUBLIC — фича `Web` при
        //     `web_allow_public`: ЛЮБОЙ публичный хост (host уже прошёл metadata/offline/deny_private —
        //     значит публичный и не metadata; allowlist не требуется). Касается ТОЛЬКО `Web`. NB: это
        //     лишь STRING-гейт; DNS-rebind (публичное имя → приватный IP) добивает РЕЗОЛВ-гард
        //     `authorize`→`check_resolved_ips(deny_private=true для Web)`, НЕ зависящий от этого флага.
        let allowed = (!feature.denies_private() && is_private_host(host))
            || (matches!(feature, EgressFeature::Web)
                && self.web_allow_public.load(Ordering::Relaxed))
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

    /// **WEB-FETCH-PUBLIC**: разрешить фиче `Web` любой ПУБЛИЧНЫЙ хост без allowlist (deny_private/SSRF/
    /// metadata/offline/redirect=none/audit сохранены). Default false. Owner-gated (`ai.web.allow_public_fetch`).
    pub fn set_web_allow_public(&self, on: bool) {
        self.web_allow_public.store(on, Ordering::Relaxed);
    }

    /// Текущее состояние WEB-FETCH-PUBLIC (для индикации/тестов).
    pub fn web_allow_public(&self) -> bool {
        self.web_allow_public.load(Ordering::Relaxed)
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
    /// R-3a (характеризация bootstrap-канона): человекочитаемый профиль клиента (какая фабрика +
    /// какие таймауты). Параметры тюнинга живут в замыкании `tune` и снаружи не наблюдаемы — эта
    /// строка делает их проверяемыми в характеризационных тестах (`debug_profile`). На рантайм не
    /// влияет; заполняется фабриками `for_*`, у [`GuardedClient::new`] — `"custom"`.
    profile: String,
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
            profile: "custom".to_string(),
        })
    }

    /// R-3a: профиль клиента (фабрика + таймауты) для характеризационных тестов bootstrap-канона —
    /// единственный способ снаружи проверить, с какими таймаутами построен guarded-клиент (сами
    /// значения спрятаны в tune-замыкании). Прод-код на это не смотрит.
    #[doc(hidden)]
    pub fn debug_profile(&self) -> &str {
        &self.profile
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
        let mut c = Self::new(policy, audit, move |b| b.connect_timeout(connect_timeout))?;
        c.profile = format!("for_chat(connect_timeout={connect_timeout:?})");
        Ok(c)
    }

    /// Профиль эмбеддинга: общий таймаут (батчи бывают тяжёлые). INFER-CFG: длительность принимается
    /// параметром (из `EmbeddingConfig::timeout()`, дефолт 60 с) — раньше был хардкод 60 с.
    pub fn for_embedding(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        timeout: Duration,
    ) -> Result<Self, NetError> {
        let mut c = Self::new(policy, audit, move |b| b.timeout(timeout))?;
        c.profile = format!("for_embedding(timeout={timeout:?})");
        Ok(c)
    }

    /// Профиль probe (проба размерности / «Проверить связь»): короткий таймаут вызывающего.
    pub fn for_probe(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        timeout: Duration,
    ) -> Result<Self, NetError> {
        let mut c = Self::new(policy, audit, move |b| b.timeout(timeout))?;
        c.profile = format!("for_probe(timeout={timeout:?})");
        Ok(c)
    }

    /// Профиль web (агент-инструменты web.search/web.fetch, EGR-AGENT): общий таймаут (страницы бывают
    /// медленные). redirect=none сохранён (core_client_builder) — анти-SSRF на редиректах.
    pub fn for_web(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        timeout: Duration,
    ) -> Result<Self, NetError> {
        let mut c = Self::new(policy, audit, move |b| b.timeout(timeout))?;
        c.profile = format!("for_web(timeout={timeout:?})");
        Ok(c)
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
            // URL без хоста (нераспарсенный): НЕ персистим сырую строку в durable-аудит — она могла бы
            // нести креды (`user:pass@`) или иной мусор от модели. Пишем безопасный плейсхолдер.
            const UNPARSEABLE: &str = "<unparseable-url>";
            self.audit
                .record(
                    feature,
                    UNPARSEABLE.to_string(),
                    bytes_out,
                    &Err(EgressDenied::HostNotAllowed(Redacted::new(
                        UNPARSEABLE.to_string(),
                    ))),
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
mod tests;
