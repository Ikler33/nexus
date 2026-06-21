//! GuardedProxy — единственный сетевой путь песочного прогона (SANDBOX-2, спека §4).
//!
//! Контейнер бежит `--network=none` (нет NIC). Сетевую capability ему даёт ТОЛЬКО host-side
//! [`GuardedProxy`] по AF_UNIX: in-sandbox-шим [`ProxyGuardedClient`] фреймит каждый запрос как typed
//! JSON-RPC (`egress/get`/`egress/post`, framing AGENT-CONNECT — [`RpcMessage`]), хост ре-эмитит его
//! через СУЩЕСТВУЮЩИЙ [`GuardedClient`] (chokepoint: allowlist → SSRF/DNS-rebind → durable audit с
//! `run_id`). Второго не-guarded пути нет физически.
//!
//! Fail-closed инварианты (§4.3):
//! - **`run_id` НЕ В ПРОТОКОЛЕ** — клиент физически не может его задать; хост всегда штампует свой
//!   (`RunCtx::run`), корреляция audit неподделываема by-construction (сильнее, чем «игнорировать поле»).
//! - **Хост назначения — только из `url`** (его парсит `GuardedClient::authorize`, не доверяя ничему
//!   из пода). Typed-верба, НЕ HTTP-forward-proxy → нет request-smuggling/desync SSRF.
//! - **deny-not-clamp**: фича запроса валидируется матчингом строки против `Display` allow-set прогона;
//!   нет совпадения (неизвестная / `probe`/`news_feed` / `web`-когда-не-разрешён) → **отказ**, а НЕ
//!   тихий выбор более мягкой фичи (Chat/Embed допускают LAN → кламп открыл бы LAN-SSRF).
//! - **Per-run egress-бюджет** (§5.6, анти-эксфильтрация): кэпы на запросы + исходящие байты (тело
//!   POST). Превышение → `RpcError` ДО сети, бэкенд не зовётся.
//! - Ошибки бэкенда **санитизированы** в `RpcError` (без host/url/кредов, §4.2).
//!
//! Бэкенд абстрагирован [`EgressBackend`] (реальный = [`GuardedClient`]) — это делает логику прокси
//! (парс/allow-set/бюджет/фрейминг) Tier-1-тестируемой БЕЗ сети (мок-бэкенд). ProxyGuardedClient НЕ
//! конструирует `reqwest` (durability-линт `scripts/check-sandbox-egress.mjs`).

use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::connect::{RpcError, RpcMessage, Transport};
use crate::net::{EgressFeature, GuardedClient, NetError, RunCtx};

/// JSON-RPC метод: GET через guarded-эгресс.
pub const EGRESS_GET: &str = "egress/get";
/// JSON-RPC метод: POST JSON-тела через guarded-эгресс.
pub const EGRESS_POST: &str = "egress/post";

/// Кастомные коды RpcError (вне зарезервированного JSON-RPC-диапазона). Сообщения — общие (анти-утечка).
const CODE_EGRESS_DENIED: i32 = -32010;
const CODE_EGRESS_FAILED: i32 = -32011;
const CODE_BUDGET: i32 = -32012;

/// Кэп тела ОТВЕТА (буферизуем; стриминг chat — рефайнмент, спека §4.2). 8 MiB — с запасом под
/// chat/embed/web JSON-ответы.
pub const DEFAULT_RESP_BODY_CAP: usize = 8 * 1024 * 1024;

/// Запрос эгресса на проводе. **БЕЗ `run_id`** (намеренно — клиент не задаёт корреляцию).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressRequest {
    /// Строка фичи (`chat`/`embed`/`web` — матчится против allow-set по `Display`, §4.3).
    pub feature: String,
    /// Полный URL (хост извлекает ХОСТ — `GuardedClient`).
    pub url: String,
    /// Тело JSON для `egress/post` (для `egress/get` — `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Ответ эгресса на проводе. `body` — UTF-8 (chat/embed/web — JSON; бинарь не поддержан, не нужен).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EgressResponse {
    /// HTTP-статус.
    pub status: u16,
    /// Whitelisted-подмножество заголовков ответа (пока только `content-type`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Тело ответа (UTF-8 lossy, кап `DEFAULT_RESP_BODY_CAP`).
    pub body: String,
}

/// Per-run бюджет эгресса (§5.6). Считает исходящие запросы + ИСХОДЯЩИЕ байты (тела POST = вектор
/// эксфильтрации). Превышение — отказ ДО сети. `u64::MAX`/`u32::MAX` = «без лимита».
#[derive(Debug)]
pub struct EgressBudget {
    byte_cap: u64,
    req_cap: u32,
    /// `(исходящие_байты, число_запросов)`.
    state: Mutex<(u64, u32)>,
}

impl EgressBudget {
    pub fn new(byte_cap: u64, req_cap: u32) -> Self {
        Self {
            byte_cap,
            req_cap,
            state: Mutex::new((0, 0)),
        }
    }

    /// Пытается учесть запрос с `out_bytes` исходящих. Проверка-ТО-фиксация: при превышении НЕ мутирует
    /// (последующие запросы тоже корректно считаются). `Err` → отказ.
    fn try_consume(&self, out_bytes: u64) -> Result<(), &'static str> {
        let mut s = self.state.lock().expect("budget mutex");
        if s.1.saturating_add(1) > self.req_cap {
            return Err("превышен лимит запросов эгресса прогона");
        }
        if s.0.saturating_add(out_bytes) > self.byte_cap {
            return Err("превышен байтовый лимит эгресса прогона");
        }
        s.0 += out_bytes;
        s.1 += 1;
        Ok(())
    }
}

/// HTTP-глагол проксируемого запроса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    Post,
}

/// Ответ бэкенда (до фрейминга в `EgressResponse`).
pub struct BackendResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Абстракция реального эгресса (за ней — [`GuardedClient`]). Вынесена ради Tier-1-тестируемости логики
/// прокси без сети (мок-бэкенд). РЕАЛЬНЫЙ путь — [`GuardedClientBackend`] — единственное, что зовёт
/// `GuardedClient` (chokepoint цел).
#[async_trait]
pub trait EgressBackend: Send + Sync {
    async fn fetch(
        &self,
        verb: Verb,
        url: &str,
        feature: EgressFeature,
        body: Option<&Value>,
        ctx: RunCtx,
    ) -> Result<BackendResponse, NetError>;
}

/// Реальный бэкенд: ре-эмитит через [`GuardedClient`] (allowlist/SSRF/audit/`run_id` — внутри него).
pub struct GuardedClientBackend {
    client: GuardedClient,
    resp_cap: usize,
}

impl GuardedClientBackend {
    pub fn new(client: GuardedClient) -> Self {
        Self {
            client,
            resp_cap: DEFAULT_RESP_BODY_CAP,
        }
    }
}

#[async_trait]
impl EgressBackend for GuardedClientBackend {
    async fn fetch(
        &self,
        verb: Verb,
        url: &str,
        feature: EgressFeature,
        body: Option<&Value>,
        ctx: RunCtx,
    ) -> Result<BackendResponse, NetError> {
        let resp = match verb {
            Verb::Get => self.client.get(url, feature, ctx).await?,
            Verb::Post => {
                let b = body.cloned().unwrap_or(Value::Null);
                self.client.post_json(url, feature, &b, ctx).await?
            }
        };
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        // Кап тела ответа (chunk-aware, как web_tools::read_capped).
        let mut body = Vec::new();
        let mut resp = resp;
        while let Some(chunk) = resp.chunk().await? {
            if body.len() + chunk.len() > self.resp_cap {
                let room = self.resp_cap.saturating_sub(body.len());
                body.extend_from_slice(&chunk[..room]);
                break;
            }
            body.extend_from_slice(&chunk);
        }
        Ok(BackendResponse {
            status,
            content_type,
            body,
        })
    }
}

/// Host-side прокси: валидирует фичу (deny-not-clamp), бюджет, штампует `run_id`, ре-эмитит через бэкенд.
pub struct GuardedProxy<B: EgressBackend> {
    backend: B,
    /// `run_id` ПРОГОНА — штампуется хостом (клиент задать не может; в `EgressRequest` поля нет).
    run_id: i64,
    /// Allow-set прогона (§5.6: по умолчанию `{Chat, Embed}`; `Web` — если `sandbox_web_enabled`).
    allowed: Vec<EgressFeature>,
    budget: EgressBudget,
}

impl<B: EgressBackend> GuardedProxy<B> {
    pub fn new(backend: B, run_id: i64, allowed: Vec<EgressFeature>, budget: EgressBudget) -> Self {
        Self {
            backend,
            run_id,
            allowed,
            budget,
        }
    }

    /// Обрабатывает один `egress/*`-запрос. `Ok(Value)` = сериализованный [`EgressResponse`];
    /// `Err(RpcError)` = санитизированный отказ (фича/бюджет/бэкенд).
    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let verb = match method {
            EGRESS_GET => Verb::Get,
            EGRESS_POST => Verb::Post,
            _ => return Err(RpcError::method_not_found()),
        };
        let req: EgressRequest =
            serde_json::from_value(params).map_err(|_| RpcError::invalid_params())?;

        // deny-not-clamp: матчим строку фичи против ALLOW-SET по Display. Нет совпадения (неизвестная /
        // probe/news_feed / web-когда-не-разрешён) → отказ. НИКОГДА не выбираем более мягкую фичу.
        let feature = self
            .allowed
            .iter()
            .copied()
            .find(|f| f.to_string() == req.feature)
            .ok_or_else(|| RpcError {
                code: CODE_EGRESS_DENIED,
                message: "фича эгресса не разрешена для песочного прогона".into(),
            })?;

        // Бюджет (§5.6): исходящие байты = длина тела POST (вектор эксфильтрации), GET = 0.
        let out_bytes = match (verb, &req.body) {
            (Verb::Post, Some(b)) => serde_json::to_vec(b).map(|v| v.len()).unwrap_or(0) as u64,
            _ => 0,
        };
        if let Err(msg) = self.budget.try_consume(out_bytes) {
            return Err(RpcError {
                code: CODE_BUDGET,
                message: msg.into(),
            });
        }

        // Ре-эмит через бэкенд с ХОСТ-штампованным run_id (клиент его не задавал).
        let ctx = RunCtx::run(self.run_id);
        let resp = self
            .backend
            .fetch(verb, &req.url, feature, req.body.as_ref(), ctx)
            .await
            .map_err(map_net_err)?;

        let framed = EgressResponse {
            status: resp.status,
            content_type: resp.content_type,
            body: String::from_utf8_lossy(&resp.body).into_owned(),
        };
        serde_json::to_value(framed).map_err(|e| RpcError::internal(e.to_string()))
    }
}

/// Санитизация `NetError` → `RpcError` (без host/url/кредов; детали — в server-лог через `internal`).
fn map_net_err(e: NetError) -> RpcError {
    match e {
        NetError::Denied(_) | NetError::BadUrl => RpcError {
            code: CODE_EGRESS_DENIED,
            message: "эгресс запрещён политикой".into(),
        },
        NetError::Http(_) => RpcError {
            code: CODE_EGRESS_FAILED,
            message: "сетевая ошибка эгресса".into(),
        },
    }
}

/// In-sandbox-шим: ТА ЖЕ поверхность вызова (`get`/`post_json`), но фреймит RPC поверх [`Transport`] к
/// host-side [`GuardedProxy`]. **НЕ конструирует `reqwest`** — нет второго не-guarded пути (линт §8.3).
/// Возвращает [`EgressResponse`]-DTO; адаптация провайдеров (chat/embed) под него — SANDBOX-4.
pub struct ProxyGuardedClient<T: Transport> {
    transport: T,
    /// Счётчик id запросов (эгресс агента последователен — по одному in-flight).
    next_id: Mutex<i64>,
}

impl<T: Transport> ProxyGuardedClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: Mutex::new(1),
        }
    }

    pub async fn get(&self, url: &str, feature: EgressFeature) -> Result<EgressResponse, RpcError> {
        self.call(
            EGRESS_GET,
            EgressRequest {
                feature: feature.to_string(),
                url: url.to_string(),
                body: None,
            },
        )
        .await
    }

    pub async fn post_json(
        &self,
        url: &str,
        feature: EgressFeature,
        body: &Value,
    ) -> Result<EgressResponse, RpcError> {
        self.call(
            EGRESS_POST,
            EgressRequest {
                feature: feature.to_string(),
                url: url.to_string(),
                body: Some(body.clone()),
            },
        )
        .await
    }

    async fn call(&self, method: &str, req: EgressRequest) -> Result<EgressResponse, RpcError> {
        let id = {
            let mut g = self.next_id.lock().expect("id mutex");
            let id = *g;
            *g += 1;
            id
        };
        let params = serde_json::to_value(&req).map_err(|e| RpcError::internal(e.to_string()))?;
        self.transport
            .send(RpcMessage::request(id, method, params))
            .await
            .map_err(|_| RpcError::internal("proxy transport send"))?;
        let msg = self
            .transport
            .recv()
            .await
            .ok_or_else(|| RpcError::internal("proxy transport closed"))?;
        match msg {
            RpcMessage::Response {
                id: resp_id,
                result,
            } => {
                // Fail-closed корреляция: ответ ДОЛЖЕН нести тот же id, что и запрос. Сегодня шим
                // последователен (один in-flight), но если будущий срез разделит его между задачами —
                // несовпадение id ловится здесь (ошибка), а не молча отдаёт чужое тело.
                if resp_id != id {
                    return Err(RpcError::internal("proxy: id ответа не совпал с запросом"));
                }
                match result {
                    Ok(v) => {
                        serde_json::from_value(v).map_err(|e| RpcError::internal(e.to_string()))
                    }
                    Err(e) => Err(e),
                }
            }
            _ => Err(RpcError::internal("proxy: ожидался Response")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::connect::channel_pair;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    /// Мок-бэкенд: записывает последний (url, feature, run_id) + возвращает канонический ответ; считает
    /// число вызовов (для проверки, что deny-пути НЕ доходят до сети).
    #[derive(Default)]
    struct MockBackend {
        calls: std::sync::atomic::AtomicUsize,
        last_url: Mutex<String>,
        last_run_id: AtomicI64,
        last_feature: Mutex<Option<EgressFeature>>,
    }

    #[async_trait]
    impl EgressBackend for MockBackend {
        async fn fetch(
            &self,
            _verb: Verb,
            url: &str,
            feature: EgressFeature,
            _body: Option<&Value>,
            ctx: RunCtx,
        ) -> Result<BackendResponse, NetError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_url.lock().unwrap() = url.to_string();
            *self.last_feature.lock().unwrap() = Some(feature);
            self.last_run_id
                .store(ctx.run_id.unwrap_or(-1), Ordering::SeqCst);
            Ok(BackendResponse {
                status: 200,
                content_type: Some("application/json".into()),
                body: br#"{"ok":true}"#.to_vec(),
            })
        }
    }

    fn proxy(
        backend: Arc<MockBackend>,
        allowed: Vec<EgressFeature>,
    ) -> GuardedProxy<Arc<MockBackend>> {
        GuardedProxy::new(backend, 42, allowed, EgressBudget::new(1_000_000, 100))
    }

    #[async_trait]
    impl EgressBackend for Arc<MockBackend> {
        async fn fetch(
            &self,
            verb: Verb,
            url: &str,
            feature: EgressFeature,
            body: Option<&Value>,
            ctx: RunCtx,
        ) -> Result<BackendResponse, NetError> {
            (**self).fetch(verb, url, feature, body, ctx).await
        }
    }

    #[tokio::test]
    async fn post_success_stamps_host_run_id_and_frames_response() {
        let mock = Arc::new(MockBackend::default());
        let p = proxy(
            mock.clone(),
            vec![EgressFeature::Chat, EgressFeature::Embed],
        );
        let params = serde_json::to_value(EgressRequest {
            feature: "chat".into(),
            url: "https://llm.example/v1/chat".into(),
            body: Some(serde_json::json!({"q": 1})),
        })
        .unwrap();
        let out = p.handle(EGRESS_POST, params).await.unwrap();
        let resp: EgressResponse = serde_json::from_value(out).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, r#"{"ok":true}"#);
        // host-штампованный run_id (в запросе его НЕ было), хост-из-url передан бэкенду.
        assert_eq!(mock.last_run_id.load(Ordering::SeqCst), 42);
        assert_eq!(
            &*mock.last_url.lock().unwrap(),
            "https://llm.example/v1/chat"
        );
        assert_eq!(
            *mock.last_feature.lock().unwrap(),
            Some(EgressFeature::Chat)
        );
    }

    #[tokio::test]
    async fn unknown_feature_denied_backend_untouched() {
        let mock = Arc::new(MockBackend::default());
        let p = proxy(mock.clone(), vec![EgressFeature::Chat]);
        for f in ["bogus", "probe", "news_feed"] {
            let params = serde_json::to_value(EgressRequest {
                feature: f.into(),
                url: "https://x/y".into(),
                body: None,
            })
            .unwrap();
            assert!(
                p.handle(EGRESS_GET, params).await.is_err(),
                "{f} должно отвергаться"
            );
        }
        assert_eq!(
            mock.calls.load(Ordering::SeqCst),
            0,
            "deny не доходит до сети"
        );
    }

    #[tokio::test]
    async fn over_broad_feature_denied_not_clamped() {
        // web запрошен, но allow-set = {Chat,Embed} → deny (НЕ кламп в Chat, который пустил бы LAN).
        let mock = Arc::new(MockBackend::default());
        let p = proxy(
            mock.clone(),
            vec![EgressFeature::Chat, EgressFeature::Embed],
        );
        let params = serde_json::to_value(EgressRequest {
            feature: "web".into(),
            url: "https://x/y".into(),
            body: None,
        })
        .unwrap();
        assert!(p.handle(EGRESS_GET, params).await.is_err());
        assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn web_allowed_when_in_allowset() {
        let mock = Arc::new(MockBackend::default());
        let p = proxy(mock.clone(), vec![EgressFeature::Chat, EgressFeature::Web]);
        let params = serde_json::to_value(EgressRequest {
            feature: "web".into(),
            url: "https://x/y".into(),
            body: None,
        })
        .unwrap();
        assert!(p.handle(EGRESS_GET, params).await.is_ok());
        assert_eq!(*mock.last_feature.lock().unwrap(), Some(EgressFeature::Web));
    }

    #[tokio::test]
    async fn budget_req_cap_denies() {
        let mock = Arc::new(MockBackend::default());
        let p = GuardedProxy::new(
            mock.clone(),
            7,
            vec![EgressFeature::Chat],
            EgressBudget::new(u64::MAX, 2), // 2 запроса
        );
        let mk = || {
            serde_json::to_value(EgressRequest {
                feature: "chat".into(),
                url: "https://x/y".into(),
                body: None,
            })
            .unwrap()
        };
        assert!(p.handle(EGRESS_GET, mk()).await.is_ok());
        assert!(p.handle(EGRESS_GET, mk()).await.is_ok());
        assert!(
            p.handle(EGRESS_GET, mk()).await.is_err(),
            "3-й сверх req_cap"
        );
        assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn budget_byte_cap_denies_large_post() {
        let mock = Arc::new(MockBackend::default());
        let p = GuardedProxy::new(
            mock.clone(),
            7,
            vec![EgressFeature::Chat],
            EgressBudget::new(16, u32::MAX), // 16 байт исходящих
        );
        let big = serde_json::json!({"data": "x".repeat(100)});
        let params = serde_json::to_value(EgressRequest {
            feature: "chat".into(),
            url: "https://x/y".into(),
            body: Some(big),
        })
        .unwrap();
        assert!(
            p.handle(EGRESS_POST, params).await.is_err(),
            "тело > byte_cap"
        );
        assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unknown_method_not_found() {
        let mock = Arc::new(MockBackend::default());
        let p = proxy(mock, vec![EgressFeature::Chat]);
        assert!(p.handle("egress/delete", Value::Null).await.is_err());
    }

    #[tokio::test]
    async fn shim_roundtrip_over_channel_transport() {
        // ProxyGuardedClient (in-sandbox) ↔ ChannelTransport ↔ GuardedProxy (host-side, mock backend).
        let (client_t, host_t) = channel_pair();
        let mock = Arc::new(MockBackend::default());
        let p = proxy(mock.clone(), vec![EgressFeature::Chat]);

        // Host-loop: одна итерация — принять запрос, обработать, ответить.
        let host = tokio::spawn(async move {
            let msg = host_t.recv().await.unwrap();
            if let RpcMessage::Request { id, method, params } = msg {
                let result = p.handle(&method, params).await;
                host_t
                    .send(RpcMessage::Response { id, result })
                    .await
                    .unwrap();
            }
        });

        let shim = ProxyGuardedClient::new(client_t);
        let resp = shim
            .post_json(
                "https://llm/v1",
                EgressFeature::Chat,
                &serde_json::json!({"q": 1}),
            )
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, r#"{"ok":true}"#);
        assert_eq!(resp.content_type.as_deref(), Some("application/json"));
        assert_eq!(mock.last_run_id.load(Ordering::SeqCst), 42);
        host.await.unwrap();
    }

    #[tokio::test]
    async fn shim_propagates_rpc_error() {
        let (client_t, host_t) = channel_pair();
        let mock = Arc::new(MockBackend::default());
        let p = proxy(mock, vec![EgressFeature::Chat]);
        let host = tokio::spawn(async move {
            let msg = host_t.recv().await.unwrap();
            if let RpcMessage::Request { id, method, params } = msg {
                let result = p.handle(&method, params).await;
                host_t
                    .send(RpcMessage::Response { id, result })
                    .await
                    .unwrap();
            }
        });
        let shim = ProxyGuardedClient::new(client_t);
        // web не в allow-set → host вернёт RpcError → шим пробрасывает Err.
        let r = shim.get("https://x/y", EgressFeature::Web).await;
        assert!(r.is_err());
        host.await.unwrap();
    }

    #[tokio::test]
    async fn shim_rejects_mismatched_response_id() {
        // Fail-closed: ответ с ЧУЖИМ id → шим возвращает Err (не отдаёт тело как «своё»).
        let (client_t, host_t) = channel_pair();
        let host = tokio::spawn(async move {
            let _req = host_t.recv().await.unwrap();
            // Отвечаем с заведомо НЕ тем id (запрос шлёт id=1).
            host_t
                .send(RpcMessage::Response {
                    id: serde_json::json!(9999),
                    result: Ok(serde_json::json!({"status": 200, "body": "x"})),
                })
                .await
                .unwrap();
        });
        let shim = ProxyGuardedClient::new(client_t);
        let r = shim.get("https://x/y", EgressFeature::Chat).await;
        assert!(r.is_err(), "несовпадение id ответа должно быть ошибкой");
        host.await.unwrap();
    }
}
