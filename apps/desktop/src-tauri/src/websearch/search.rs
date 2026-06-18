//! SearXNG-клиент web-агента (W3/W4). Запрос идёт через `net::GuardedClient` с
//! `EgressFeature::Web` и тем же DNS-rebinding-гардом, что у ленты (resolve→проверка всех IP→пин
//! проверенного адреса в клиент). Лимиты W3: таймаут 20 с, body-cap 2 МБ. W4: исходящий
//! поисковый запрос сканируется `git::scan_secrets` ДО отправки — найден секрет → запрос НЕ уходит.
//!
//! Возвращаются нормализованные [`SearchResult`] (title/url/snippet). Agent-loop (≤3 поиска на ход,
//! anti-injection обёртка результатов, tool-use запрещён) — срез W-2.

use std::net::SocketAddr;
use std::sync::Arc;

use serde::Deserialize;

use crate::git::scan_secrets;
use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient, Resolver};
use crate::news::{check_resolved_ips, read_body_capped, FEED_BODY_CAP};

/// W3: таймаут поискового запроса и потолок тела ответа (как у ленты — единый web-класс).
const SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// Сколько результатов берём из ответа SearXNG (срез контекста для LLM, не лимит фетча).
pub const MAX_RESULTS: usize = 6;

/// Нормализованный результат поиска (для контекста agent-loop и цитат-источников).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    /// Сниппет SearXNG (`content`) — недоверенный текст; agent-loop обернёт его anti-injection.
    pub snippet: String,
}

/// Сырой ответ SearXNG `?format=json` (берём только нужные поля).
#[derive(Deserialize)]
struct SearxResponse {
    #[serde(default)]
    results: Vec<SearxResult>,
}
#[derive(Deserialize)]
struct SearxResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

/// Ошибка поиска: типизирована, чтобы agent-loop отличал «секрет в запросе» (W4, запрос не ушёл)
/// от сетевой ошибки/отказа политики.
#[derive(Debug)]
pub enum SearchError {
    /// W4: исходящий запрос содержит секрет → НЕ отправлен.
    SecretInQuery,
    /// Web-агент не сконфигурирован (нет URL SearXNG / фича выключена).
    NotConfigured,
    /// Сеть/политика/парсинг — текст без секретных деталей.
    Failed(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::SecretInQuery => {
                f.write_str("поисковый запрос содержит секрет — не отправлен")
            }
            SearchError::NotConfigured => f.write_str("web-агент не настроен (нет URL SearXNG)"),
            SearchError::Failed(m) => write!(f, "поиск не удался: {m}"),
        }
    }
}

/// Абстракция поиска для agent-loop (W-2) — чтобы оркестрацию тестировать с мок-поисковиком,
/// не поднимая SearXNG. Прод-реализация — [`WebSearcher`].
#[async_trait::async_trait]
pub trait Searcher: Send + Sync {
    /// `fresh` — вопрос про текущее положение дел (план `FRESH:`): выдача ограничивается свежим
    /// периодом, чтобы не отвечать по многолетним страницам.
    async fn search(&self, query: &str, fresh: bool) -> Result<Vec<SearchResult>, SearchError>;
}

#[async_trait::async_trait]
impl Searcher for WebSearcher {
    async fn search(&self, query: &str, fresh: bool) -> Result<Vec<SearchResult>, SearchError> {
        WebSearcher::search(self, query, fresh).await
    }
}

/// Web-поисковик через SearXNG. На каждый запрос: W4-скан → резолв → DNS-гард → guarded-GET с пином.
pub struct WebSearcher {
    policy: Arc<EgressPolicy>,
    audit: Arc<EgressAudit>,
    resolver: Arc<dyn Resolver>,
    /// База инстанса SearXNG (хост уже в allowlist скоупа "web" по consent, W2).
    base_url: String,
}

impl WebSearcher {
    pub fn new(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        resolver: Arc<dyn Resolver>,
        base_url: String,
    ) -> Self {
        Self {
            policy,
            audit,
            resolver,
            base_url,
        }
    }

    /// Один поиск: до [`MAX_RESULTS`] результатов. W4: запрос сканируется на секреты ДО сети.
    /// `fresh` → выдача ограничивается [`FRESH_TIME_RANGE`].
    pub async fn search(&self, query: &str, fresh: bool) -> Result<Vec<SearchResult>, SearchError> {
        // W4 (AC-SEC-3 на egress-payload): секрет в исходящем запросе → запрос НЕ уходит.
        if !scan_secrets(query).is_empty() {
            return Err(SearchError::SecretInQuery);
        }
        if self.base_url.trim().is_empty() {
            return Err(SearchError::NotConfigured);
        }

        let url = build_search_url(&self.base_url, query, fresh)?;
        let host = url
            .host_str()
            .ok_or_else(|| SearchError::Failed("URL без хоста".into()))?
            .to_string();

        // Быстрый отказ политики ДО DNS (выключенная фича/офлайн/не в allowlist) — без сети.
        self.policy
            .check(&host, EgressFeature::Web)
            .map_err(|e| SearchError::Failed(e.to_string()))?;

        // DNS-rebinding-гард (W-аддендум): резолв → проверка ВСЕХ IP → пин первого в клиент.
        let ips = self
            .resolver
            .resolve(&host)
            .await
            .map_err(|e| SearchError::Failed(format!("dns: {e}")))?;
        check_resolved_ips(&host, &ips).map_err(SearchError::Failed)?;
        let pinned = SocketAddr::new(ips[0], url.port_or_known_default().unwrap_or(443));
        let host_for_pin = host.clone();

        // Тот же резолвер инъектится в core-`GuardedClient` (P0-a): его собственный гард работает
        // поверх ТОГО ЖЕ резолва, единый [`check_resolved_ips`] — один источник истины.
        let client = GuardedClient::new(self.policy.clone(), self.audit.clone(), move |b| {
            b.timeout(SEARCH_TIMEOUT)
                .resolve_to_addrs(&host_for_pin, &[pinned])
        })
        .map_err(|e| SearchError::Failed(e.to_string()))?
        .with_resolver(self.resolver.clone());
        let resp = client
            .get(url.as_str(), EgressFeature::Web)
            .await
            .map_err(|e| SearchError::Failed(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SearchError::Failed(format!("статус {}", resp.status())));
        }
        let body = read_body_capped(resp, FEED_BODY_CAP)
            .await
            .map_err(SearchError::Failed)?;
        parse_searx(&body)
    }
}

/// SearXNG `time_range` для fresh-запросов: «год» отсекает многолетние страницы (наблюдение
/// live-smoke 2026-06-11: «последняя версия Python» отвечала по статье 2023-го), не урезая
/// recall до новостной ленты. Движки без поддержки time_range параметр игнорируют.
const FRESH_TIME_RANGE: &str = "year";

/// Строит URL запроса к SearXNG (без сети — юнит-тестируемо): путь /search (consent-URL может быть
/// и базой, и /search), `q`+`format=json`, при `fresh` — `time_range`.
fn build_search_url(base_url: &str, query: &str, fresh: bool) -> Result<reqwest::Url, SearchError> {
    let mut url = reqwest::Url::parse(base_url.trim_end_matches('/'))
        .map_err(|_| SearchError::Failed("некорректный URL SearXNG".into()))?;
    if !url.path().trim_end_matches('/').ends_with("search") {
        url.set_path(&format!("{}/search", url.path().trim_end_matches('/')));
    }
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "json");
    if fresh {
        url.query_pairs_mut()
            .append_pair("time_range", FRESH_TIME_RANGE);
    }
    Ok(url)
}

/// Парсит ответ SearXNG → нормализованные результаты (пустые url отбрасываем, обрезаем до MAX).
pub fn parse_searx(body: &str) -> Result<Vec<SearchResult>, SearchError> {
    let parsed: SearxResponse =
        serde_json::from_str(body).map_err(|e| SearchError::Failed(format!("json: {e}")))?;
    Ok(parsed
        .results
        .into_iter()
        .filter(|r| !r.url.trim().is_empty())
        .take(MAX_RESULTS)
        .map(|r| SearchResult {
            title: if r.title.trim().is_empty() {
                r.url.clone()
            } else {
                r.title
            },
            url: r.url,
            snippet: r.content,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn w4_secret_in_query_blocks_before_network() {
        // Web-поисковик с фейковым резолвером — но до сети не дойдём: секрет ловится первым.
        struct NopeResolver;
        #[async_trait::async_trait]
        impl Resolver for NopeResolver {
            async fn resolve(&self, _h: &str) -> std::io::Result<Vec<std::net::IpAddr>> {
                panic!("резолв не должен вызываться: запрос с секретом не уходит")
            }
        }
        let policy = Arc::new(EgressPolicy::new(Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )));
        let searcher = WebSearcher::new(
            policy,
            Arc::new(EgressAudit::default()),
            Arc::new(NopeResolver),
            "https://searx.example.com".into(),
        );
        // Плейсхолдер из gitleaks-allowlist (.gitleaks.toml), но `detect_token` его ловит как github-PAT.
        let q = "ghp_0123456789012345678901234567890123ab";
        let err = futures::executor::block_on(searcher.search(q, false));
        assert!(matches!(err, Err(SearchError::SecretInQuery)));
    }

    /// fresh-план ограничивает выдачу свежим периодом; обычный запрос time_range не несёт.
    /// Путь /search достраивается и для базового consent-URL, и для уже полного.
    #[test]
    fn build_search_url_adds_time_range_only_when_fresh() {
        let u =
            build_search_url("https://searx.example.com", "последняя версия python", true).unwrap();
        assert_eq!(u.path(), "/search");
        assert!(u.query().unwrap().contains("time_range=year"));
        assert!(u.query().unwrap().contains("format=json"));

        let u =
            build_search_url("https://searx.example.com/search", "обычный запрос", false).unwrap();
        assert_eq!(u.path(), "/search");
        assert!(!u.query().unwrap().contains("time_range"));
    }

    #[test]
    fn parse_searx_normalizes_and_caps() {
        let body = r#"{"results":[
            {"title":"A","url":"https://a.test","content":"snippet a"},
            {"title":"","url":"https://b.test","content":"snippet b"},
            {"title":"no-url","url":"","content":"dropped"}
        ]}"#;
        let res = parse_searx(body).unwrap();
        assert_eq!(res.len(), 2, "пустой url отброшен");
        assert_eq!(res[0].title, "A");
        assert_eq!(res[1].title, "https://b.test", "пустой title → url");
    }

    #[test]
    fn parse_searx_respects_max_results() {
        let items: Vec<String> = (0..20)
            .map(|i| format!(r#"{{"title":"t{i}","url":"https://x{i}.test","content":"c"}}"#))
            .collect();
        let body = format!(r#"{{"results":[{}]}}"#, items.join(","));
        let res = parse_searx(&body).unwrap();
        assert_eq!(res.len(), MAX_RESULTS);
    }

    #[test]
    fn parse_searx_bad_json_is_typed_error() {
        assert!(matches!(
            parse_searx("not json"),
            Err(SearchError::Failed(_))
        ));
    }
}
