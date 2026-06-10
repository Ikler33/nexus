# Egress-контроль ядра (ADR-005-ext)

> Единый chokepoint исходящего HTTP **ядра** — модуль `net/`. Каждый core-эгресс (chat/embed/probe, в
> будущем web/cloud/News Feed) ОБЯЗАН идти через `net::GuardedClient`; голый `reqwest::Client::builder` /
> `core_client_builder` вне `net/` запрещён CI-grep-линтом (`AC-EGR-1`). Решения owner-codesign —
> `docs/reviews/ADR_CODESIGN.md` (E1–E10 фундамент, W1–W4 web-аддендум); ADR — `ARCHITECTURE.md §0`
> (Egress-граница ядра, расширение ADR-005). Критерии — `AC-EGR-1..14`. Строится срезами.

## Статус по срезам

| Срез | Что | Статус |
|---|---|---|
| **1. Фундамент** | модуль `net/`: `GuardedClient` (оборачивает приватизированный `core_client_builder`, redirect=none) + `EgressPolicy` (kill-switch→feature opt-in→host-allowlist) + `EgressAudit` (in-memory append-only, `Redacted` host) + `EgressFeature{Chat,Embed,Probe}` + `EgressDenied` + предикат `blocks_cloud_metadata`; провайдеры chat/embed/probe/`test_ai_connection` через guarded; composition-root `AIClient{chat,chat_fast,chat_util,embedder,policy}`; kill-switch `AtomicBool` в `AppState`; CI-grep-линт `check-egress.mjs` | ✅ 2026-06-10 (PR-B, AC-EGR-1..13) |
| **2. UI/контроль** | тоггл «офлайн» + per-feature opt-in в настройках; чат-бейдж local/cloud/offline (E9); i18n-рендер `EgressDenied` (RU/EN, `AC-EGR-14`); персист политики в OS config-dir (E5) | 🚧 частично (2026-06-10): **бэкенд+настройки ✅** — персист E5 (`egress.json` в OS config-dir, fail-safe-дефолты), команды `get_egress_state`/`set_egress_offline`/`set_egress_feature`, блок «Сеть (egress)» в «AI / Модели» (применяется мгновенно). **Закрыто DP-12 (2026-06-11):** чат-бейдж E9 (шапка AI-панели: «Локально»/«Офлайн» из get_egress_state; «Облако» — со срезом 3) + i18n-рендер отказов AC-EGR-14 (`ChatStreamEvent::Error.denied_kind` → баннер RU/EN). Срез 2 целиком ✅ |
| **3. Cloud-fallback** | `EgressFeature::CloudFallback` + `chat_fallback`/`guard_first_token` (фасад §4.3 «план»); API-ключ в keychain; индикатор ☁ | ⏳ vision |
| **4. Web-агент / News Feed** | `EgressFeature::{Web,NewsFeed}`; SearXNG-host (consent-on-save, W2); лимиты W3 (≤3 поиска/ход, body-cap 2 MB, timeout 20 с, News Feed раз/сутки); outbound `scan_secrets` (W4); DNS-rebinding-гард для доменов; untrusted-канал web-контента (anti-injection, tool-use заблокирован) | 🚧 **News-Feed-половина ✅** (NF-4, 2026-06-10, спека `docs/specs/news-feed.md`): `EgressFeature::NewsFeed` (дефолт ВЫКЛ, consent = `news.json`; `allow_private=false`), "news"-скоуп allowlist, лимиты 20 с / 2 МБ, **DNS-гард с пином проверенного IP** (resolve-then-connect-check без TOCTOU), anti-injection-маркеры на фид-контент, W4 n/a (GET без тел). **Остаётся:** web-агент чата / `Web`-фича / SearXNG (инфра владельца) / outbound scan_secrets для промптов |

## Дизайн фундамента (срез 1)

```text
src-tauri/src/net/mod.rs
  pub struct GuardedClient { inner: reqwest::Client, policy: Arc<EgressPolicy>, audit: Arc<EgressAudit> }
  pub enum   EgressFeature { Chat, Embed, Probe }            // Web/NewsFeed/CloudFallback — позже, с фичей
  pub enum   EgressDenied  { Offline, FeatureNotEnabled(EgressFeature), HostNotAllowed(Redacted<String>) }
  #[cfg(test)] pub fn unchecked() -> GuardedClient           // мок-серверы без живого allowlist
```

**Инварианты (синтез codesign, не owner-развилки):**

1. `inner` — из **приватизированного** `core_client_builder()` (переезжает деталью внутрь `net/`), `redirect(none)` сохранён (`AC-EGR-7`). Снаружи `net/` его вызов запрещён линтом.
2. **`policy.check(host, feature)` per-request**, порядок: предикат metadata (`169.254.169.254` → reject ВСЕГДА, E7/`AC-EGR-12`) → kill-switch «офлайн» (публичный хост → `Offline`; LAN/loopback живут, E2/`AC-EGR-3`) → feature opt-in (`FeatureNotEnabled`, E6/`AC-EGR-5`) → host ∈ allowlist **ИЛИ** `is_private_host` (local-first для Chat/Embed/Probe; `HostNotAllowed` иначе, `AC-EGR-2`). `is_private_host` — из ре-экспорта `plugin/mod.rs` (НЕ дублируется, `AC-EGR-8`).
3. **`EgressAudit` — отдельный тип** (ось `feature/host/bytes_out?/decision`), НЕ слияние с брокерским `AuditEntry`. Инвариант append-only: приватный `record()`, публичны `entries()`/`len()`/`is_empty()`; host через `Redacted` (`AC-EGR-4`). `bytes_out` — best-effort `Option` (тело **запроса**: для не-стрим `Some(Content-Length)`, для chat-стрима `None`/`Some(len(messages))`; `AC-EGR-10`).
4. **kill-switch — новое `AtomicBool` в `AppState`** (`chat_cancel` — это `Mutex<Option<Arc<AtomicBool>>>`, не оно). На активном стриме «офлайн» ВЗВОДИТ существующий `chat_cancel` (`cancel_active_chat`), переиспользуя per-chunk `cancel.load()` — никакого нового механизма отмены (E10/`AC-EGR-11`).
5. Провайдеры принимают `&GuardedClient` + feature-тег вместо построения своего клиента (`OpenAiChatProvider`/`OpenAiEmbedder`/`probe_dim`); `test_ai_connection` и `probe_dim` → `Feature::Probe` (`AC-EGR-6`).
6. **Composition-root:** `GuardedClient` строится в `build_rag`/`build_chat`/`build_util_chat` от ЕДИНОГО `policy`+`audit` приложения (`AppState`); **`AIClient`** = тонкий фасад `{chat, chat_fast, chat_util, embedder, policy}` (заменяет ЧЕТЫРЕ `Arc` в `VaultContext` одним полем; решение владельца 2026-06-10 — код ушёл вперёд исходных «двух Arc» дока), БЕЗ cloud-fallback. Hot-swap chat / cold embedder сохраняются (`AC-EGR-13`).
7. **CI-grep-линт** (`AC-EGR-1`): `reqwest::Client::builder`/`core_client_builder` вне `net/` запрещён; WHITELIST — сам `net/` + `dispatch_net` (plugin net.fetch, своя политика, миграция вне скоупа, с комментарием).

**Замок durability — это chokepoint + grep-линт, а НЕ enum:** `Feature` не привязан к назначению (код может взять `Feature::Chat` для web); единую точку гарантирует линт.

**Дефолты фундамента (трактовка E4, подтверждена владельцем 2026-06-10):** kill-switch «офлайн» по умолчанию ВЫКЛЮЧЕН (это ручной режим пользователя, UI — срез 2); «egress OFF из коробки» обеспечивает **пустой allowlist**: чистый `local.json` не открывает ни одного публичного хоста. Явные `ai.chat/ai.embedding/ai.fast`-хосты из `local.json` — **авто-в-allowlist** (E4/E3 «явные `ai.*.url` разрешены»; пересборка на open-vault и в `set_ai_config`). Consent на pull-changed base_url — срез 2 (нужен персист политики E5). Фичи Chat/Embed/Probe включены (local-first, opt-in-состояния пока нет — E5).

## Что НЕ в фундаменте (явные отсрочки, «no silent caps» → BACKLOG)

- **Персист политики в OS config-dir (E5)** — для Chat/Embed/Probe-to-LAN нет opt-in-состояния для сохранения (local-first, всегда on); файл политики появляется со срезом 2/3, когда возникнет cloud/web opt-in, переживающий рестарт.
- **Plugin `net.fetch`** (`dispatch_net`) — своя политика (allowlist + `is_private_host`, 15 с); миграция в `net/` вне скоупа (whitelist-исключение линта).
- **git-sync** (libgit2-транспорт, не reqwest) — egress-политика для него отдельно (срез 2+, по host из remote-URL).
- **Web/cloud-лимиты, outbound secret-scan, DNS-rebinding, body-cap** — со срезом 4 (web-фича), per W2–W4.
