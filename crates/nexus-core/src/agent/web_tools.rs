//! EGR-AGENT: веб-инструменты агента — `web.search` (мета-поиск через SearXNG) + `web.fetch` (GET
//! публичного URL). Весь эгресс — через [`GuardedClient`] с [`EgressFeature::Web`] (web-класс:
//! `deny_private=true` → SSRF/DNS-rebind-гард внутри `get`, durable-аудит, per-call [`RunCtx`]). Результат
//! инструмента — НЕДОВЕРЕННЫЕ ДАННЫЕ: цикл фенсит его в anti-injection-маркер (I-5/AC-SEC-7) — здесь
//! инструмент лишь возвращает текст. Включается конфигом (composition root); read-only — не требует
//! actuator-флага.
//!
//! NB по безопасности: запрос `web.search` уходит на SearXNG (узел владельца), но всё же эгресс →
//! лёгкая проверка `looks_secretish` блокирует очевидные секреты в запросе ДО сети (полный
//! `git::scan_secrets` живёт в desktop; его подъём в ядро — follow-up). `web.fetch` — только публичные
//! хосты (web-класс рубит приватные IP), arbitrary-URL под egress-allowlist политики.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient, RunCtx};

use super::tool::{Tool, ToolError, ToolSpec};

/// Максимум результатов поиска, отдаваемых модели (бюджет контекста).
const MAX_RESULTS: usize = 8;
/// `time_range` для «свежего» поиска (SearXNG; движки без поддержки игнорируют).
const FRESH_TIME_RANGE: &str = "year";
/// Кап тела ответа (анти-OOM/анти-токен-флуд): и для SearXNG-JSON, и для web.fetch.
const BODY_CAP: usize = 1 << 20; // 1 MiB

/// Конфиг веб-инструментов (RUN-независимый): guarded-клиент (сконфигурён под web-таймауты) + URL
/// SearXNG. Строит композиционный корень (agentd), когда веб включён в конфиге. Per-run [`RunCtx`]
/// добавляется при сборке инструментов в [`web_tools`].
#[derive(Clone)]
pub struct WebToolsConfig {
    /// Guarded-клиент для web-эгресса (политика уже включает `EgressFeature::Web` + allowlist хостов).
    pub client: GuardedClient,
    /// База SearXNG (consent-URL). `None` → `web.search` НЕ регистрируется (остаётся только `web.fetch`).
    pub searxng_url: Option<String>,
}

/// Композиционный корень: ВКЛЮЧАЕТ web-эгресс в политике (фича `Web` + allowlist хоста SearXNG в
/// скоупе `"web"`) и строит [`WebToolsConfig`] (guarded-клиент `for_web` + URL). `None` — битый URL (без
/// хоста). Зовётся, когда `ai.web.enabled` (см. agentd). Идемпотентно по политике (повторный вызов
/// перезапишет allowlist `"web"` тем же хостом). Таймаут — общий web (страницы бывают медленные).
/// `allow_public_fetch` (WEB-FETCH-PUBLIC, owner-gated) → `web.fetch` к ЛЮБОМУ публичному хосту
/// (deny_private/SSRF/audit сохранены); default-канал (false) — только allowlist (SearXNG).
pub fn enable_web_tools(
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
    searxng_url: &str,
    timeout: std::time::Duration,
    allow_public_fetch: bool,
) -> Option<WebToolsConfig> {
    let host = reqwest::Url::parse(searxng_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))?;
    // Порядок: ставим allowlist + web_allow_public ДО включения фичи `Web`, чтобы не было окна, где
    // `Web` уже жив, а флаги ещё стартовые (для будущих рантайм-перетогглов; сейчас зовётся на старте).
    policy.set_scoped_allowlist("web", [host]);
    policy.set_web_allow_public(allow_public_fetch);
    policy.set_feature_enabled(EgressFeature::Web, true);
    let client = GuardedClient::for_web(policy.clone(), audit.clone(), timeout).ok()?;
    Some(WebToolsConfig {
        client,
        searxng_url: Some(searxng_url.to_string()),
    })
}

/// Собирает run-scoped веб-инструменты (захватывают `ctx` прогона для корреляции эгресса в аудите).
/// Пусто никогда не возвращает `web.fetch` (всегда есть); `web.search` — только при заданном SearXNG.
pub fn web_tools(cfg: &WebToolsConfig, ctx: RunCtx) -> Vec<Arc<dyn Tool>> {
    let mut v: Vec<Arc<dyn Tool>> = Vec::new();
    if let Some(url) = &cfg.searxng_url {
        v.push(Arc::new(WebSearchTool {
            client: cfg.client.clone(),
            searxng_url: url.clone(),
            ctx,
        }));
    }
    v.push(Arc::new(WebFetchTool {
        client: cfg.client.clone(),
        ctx,
    }));
    v
}

// ── web.search ────────────────────────────────────────────────────────────────────────────────────

struct WebSearchTool {
    client: GuardedClient,
    searxng_url: String,
    ctx: RunCtx,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    /// Ограничить недавними результатами (`time_range`).
    #[serde(default)]
    fresh: bool,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web.search".to_string(),
            description:
                "Поиск в интернете (мета-поиск). Возвращает список: заголовок, URL, сниппет. \
                          Используй для актуальной информации, которой нет в заметках."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Поисковый запрос" },
                    "fresh": { "type": "boolean", "description": "Только недавние результаты" }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: SearchArgs =
            serde_json::from_str(args).map_err(|e| ToolError::BadArgs(e.to_string()))?;
        let q = a.query.trim();
        if q.is_empty() {
            return Err(ToolError::BadArgs("пустой запрос".into()));
        }
        if looks_secretish(q) {
            return Err(ToolError::Exec(
                "запрос похож на секрет (токен/ключ) — НЕ отправлен в сеть".into(),
            ));
        }
        let url = build_search_url(&self.searxng_url, q, a.fresh)
            .map_err(|e| ToolError::Exec(format!("URL SearXNG: {e}")))?;
        let resp = self
            .client
            .get(&url, EgressFeature::Web, self.ctx)
            .await
            .map_err(|e| ToolError::Exec(format!("egress: {e}")))?;
        if !resp.status().is_success() {
            return Err(ToolError::Exec(format!("SearXNG HTTP {}", resp.status())));
        }
        let body = read_capped(resp, BODY_CAP).await.map_err(ToolError::Exec)?;
        let results = parse_searx(&body).map_err(ToolError::Exec)?;
        if results.is_empty() {
            return Ok("(нет результатов)".into());
        }
        let text = results
            .iter()
            .enumerate()
            .map(|(i, r)| format!("{}. {}\n   {}\n   {}", i + 1, r.title, r.url, r.snippet))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(text)
    }
}

// ── web.fetch ─────────────────────────────────────────────────────────────────────────────────────

struct WebFetchTool {
    client: GuardedClient,
    ctx: RunCtx,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchArgs {
    url: String,
}

#[async_trait]
impl Tool for WebFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web.fetch".to_string(),
            description:
                "Загрузить текстовое содержимое публичного веб-URL (HTML очищается до текста). \
                          Только http/https, только публичные адреса."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Публичный http(s) URL" }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: FetchArgs =
            serde_json::from_str(args).map_err(|e| ToolError::BadArgs(e.to_string()))?;
        let url = a.url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ToolError::BadArgs("URL должен быть http(s)".into()));
        }
        // Креды в URL (`user:pass@`) — запрет (утечка в сеть/аудит, почти всегда ошибка модели).
        if has_url_credentials(url) {
            return Err(ToolError::BadArgs(
                "URL с встроенными кредами (user:pass@) запрещён".into(),
            ));
        }
        // Прочие секреты в URL (токены в query) → не отправляем (как web.search для запроса).
        if looks_secretish(url) {
            return Err(ToolError::Exec(
                "URL похож на секрет (токен/ключ) — НЕ отправлен в сеть".into(),
            ));
        }
        let resp = self
            .client
            .get(url, EgressFeature::Web, self.ctx)
            .await
            .map_err(|e| ToolError::Exec(format!("egress: {e}")))?;
        if !resp.status().is_success() {
            return Err(ToolError::Exec(format!("HTTP {}", resp.status())));
        }
        let body = read_capped(resp, BODY_CAP).await.map_err(ToolError::Exec)?;
        let text = html_to_text(&body);
        if text.trim().is_empty() {
            return Ok("(пустой документ)".into());
        }
        Ok(text)
    }
}

// ── Хелперы (чистые, юнит-тестируемые) ──────────────────────────────────────────────────────────

/// Строит URL запроса к SearXNG: путь `/search` (consent-URL может быть базой ИЛИ уже `/search`),
/// `q`+`format=json`, при `fresh` — `time_range`. Сравниваем ПОСЛЕДНИЙ сегмент пути (не `ends_with`,
/// иначе `/research` ложно сошёл бы за готовый эндпоинт — грабля m9 из desktop).
fn build_search_url(base_url: &str, query: &str, fresh: bool) -> Result<String, String> {
    let mut url = reqwest::Url::parse(base_url.trim_end_matches('/'))
        .map_err(|_| "некорректный URL".to_string())?;
    let trimmed = url.path().trim_end_matches('/').to_string();
    if trimmed.rsplit('/').next() != Some("search") {
        url.set_path(&format!("{trimmed}/search"));
    }
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "json");
    if fresh {
        url.query_pairs_mut()
            .append_pair("time_range", FRESH_TIME_RANGE);
    }
    Ok(url.to_string())
}

#[derive(Debug, Deserialize)]
struct SearxResponse {
    #[serde(default)]
    results: Vec<SearxResult>,
}
#[derive(Debug, Deserialize)]
struct SearxResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

/// Нормализованный результат поиска для модели.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Парсит JSON SearXNG → результаты (пустые url отбрасываем, обрезаем до [`MAX_RESULTS`]).
fn parse_searx(body: &str) -> Result<Vec<SearchResult>, String> {
    let parsed: SearxResponse =
        serde_json::from_str(body).map_err(|e| format!("json SearXNG: {e}"))?;
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

/// Читает тело ответа с КАПОМ байт (анти-OOM): по чанкам (`reqwest::Response::chunk`, без stream-фичи),
/// останавливаясь на `cap`. Лосси-UTF8.
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Result<String, String> {
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("чтение тела: {e}"))?
    {
        let room = cap.saturating_sub(buf.len());
        if room == 0 {
            break;
        }
        let take = room.min(chunk.len());
        buf.extend_from_slice(&chunk[..take]);
        if buf.len() >= cap {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Грубое HTML→текст: вырезает `<script>`/`<style>`-блоки, снимает теги, схлопывает пробелы. Не идеально
/// (без полноценного парсера), но даёт модели читаемый текст вместо разметки и экономит токены.
fn html_to_text(html: &str) -> String {
    let mut s = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase(); // регистро-независимый поиск тегов; та же длина в байтах
    let mut i = 0usize; // байтовый индекс (синхронно в html и lower)
    while i < html.len() {
        if html.as_bytes()[i] == b'<' {
            let rest_lower = &lower[i..];
            // Блоки script/style — вырезаем ЦЕЛИКОМ (вместе с содержимым).
            if let Some(tag) = ["<script", "<style"]
                .into_iter()
                .find(|t| rest_lower.starts_with(t))
            {
                let close = if tag == "<script" {
                    "</script>"
                } else {
                    "</style>"
                };
                match rest_lower.find(close) {
                    Some(end) => {
                        i += end + close.len();
                        s.push(' ');
                        continue;
                    }
                    None => break, // незакрытый блок → конец
                }
            }
            // Обычный тег: до ближайшего '>'.
            match html[i..].find('>') {
                Some(end) => {
                    i += end + 1;
                    s.push(' '); // тег → пробел (граница слов)
                    continue;
                }
                None => break, // незакрытый тег → конец
            }
        }
        // Копируем ОДИН СИМВОЛ (UTF-8-корректно) — иначе не-ASCII (кириллица) превратилась бы в мусор.
        let ch = html[i..].chars().next().expect("i — граница символа");
        s.push(ch);
        i += ch.len_utf8();
    }
    // Декод нескольких частых сущностей + схлопывание пробелов.
    let s = s
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// ЛЁГКАЯ эвристика «строка похожа на секрет» (анти-эксфил запроса в сеть). НЕ замена `git::scan_secrets`
/// (живёт в desktop; подъём в ядро — follow-up): ловит очевидные префиксы ключей + длинные high-entropy
/// токены. Цель — не пропустить грубую утечку, не блокировать обычные запросы.
/// Есть ли в строке-URL встроенные креды (`scheme://user[:pass]@host`)? Через парсер URL (надёжнее
/// строковых эвристик). Не-URL → false. Креды в URL — вектор утечки (в сеть/аудит) и почти всегда ошибка.
fn has_url_credentials(token: &str) -> bool {
    reqwest::Url::parse(token)
        .map(|u| !u.username().is_empty() || u.password().is_some())
        .unwrap_or(false)
}

fn looks_secretish(s: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "sk-",
        "ghp_",
        "gho_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "AKIA",
        "ASIA",
        "AIza",
        "glpat-",
    ];
    if PREFIXES.iter().any(|p| s.contains(p)) {
        return true;
    }
    // Basic-Auth в URL (`scheme://user:pass@host`) — креды не должны утечь в сеть/аудит.
    if s.split_whitespace().any(has_url_credentials) {
        return true;
    }
    // Длинный непрерывный токен из base64-алфавита без пробелов → вероятный ключ.
    s.split(|c: char| c.is_whitespace()).any(|tok| {
        tok.len() >= 40
            && tok.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || c == '+'
                    || c == '/'
                    || c == '_'
                    || c == '-'
                    || c == '='
            })
            && tok.chars().any(|c| c.is_ascii_digit())
            && tok.chars().any(|c| c.is_ascii_uppercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_url_appends_search_path_and_format() {
        let u = build_search_url("http://searx.example:8888", "rust async", false).unwrap();
        assert!(u.starts_with("http://searx.example:8888/search?"));
        assert!(u.contains("q=rust+async") || u.contains("q=rust%20async"));
        assert!(u.contains("format=json"));
        assert!(!u.contains("time_range"));
        // База уже с /search — не дублируем.
        let u2 = build_search_url("http://x/search", "q", true).unwrap();
        assert_eq!(u2.matches("/search").count(), 1);
        assert!(u2.contains("time_range=year"));
        // subpath /research НЕ считается готовым эндпоинтом (m9).
        let u3 = build_search_url("http://x/research", "q", false).unwrap();
        assert!(u3.contains("/research/search"));
    }

    #[test]
    fn parse_searx_normalizes_and_caps() {
        let body = r#"{"results":[
            {"title":"A","url":"http://a","content":"snip a"},
            {"title":"","url":"http://b","content":"snip b"},
            {"title":"C","url":"  ","content":"dropped (empty url)"}
        ]}"#;
        let r = parse_searx(body).unwrap();
        assert_eq!(r.len(), 2); // пустой url отброшен
        assert_eq!(r[0].title, "A");
        assert_eq!(r[1].title, "http://b"); // пустой title → url
    }

    #[test]
    fn html_to_text_strips_tags_and_scripts() {
        let h = "<html><head><style>.x{color:red}</style></head><body>Hello <b>world</b>\
                 <script>alert(1)</script> &amp; more</body></html>";
        let t = html_to_text(h);
        assert!(t.contains("Hello world"), "got: {t}");
        assert!(!t.contains("alert"), "script not stripped: {t}");
        assert!(!t.contains("color:red"), "style not stripped: {t}");
        assert!(t.contains("& more"));
    }

    #[test]
    fn html_to_text_preserves_non_ascii() {
        // UTF-8-корректность: кириллица/эмодзи НЕ должны превратиться в мусор (баг byte-as-char).
        let h = "<p>Привет, <b>мир</b> 🌍 — café</p>";
        let t = html_to_text(h);
        assert!(t.contains("Привет"), "got: {t}");
        assert!(t.contains("мир"), "got: {t}");
        assert!(t.contains("café"), "got: {t}");
        assert!(t.contains('🌍'), "got: {t}");
    }

    #[test]
    fn secretish_blocks_keys_not_normal_queries() {
        // Фикстуры собираем РАНТАЙМОМ (префикс отдельно от тела): в исходнике нет цельного секрет-
        // паттерна → secret-сканер (gitleaks) не флагает тест, а looks_secretish видит полную строку.
        let sk = format!("my key is sk-{}", "ABCDEF1234567890");
        let ghp = format!("ghp_{}", "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww");
        let akia = format!("AKIA{}", "IOSFODNN7EXAMPLE");
        let tok = format!("Zm9vYmFy{}", "0000ABCDEFabcdefXYZ9876543210ABCDQ");
        let basic = format!("see http://admin:{}@internal/wiki", "hunter2");
        assert!(looks_secretish(&sk));
        assert!(looks_secretish(&ghp));
        assert!(looks_secretish(&akia));
        assert!(looks_secretish(&tok)); // длинный high-entropy токен
        assert!(looks_secretish(&basic)); // basic-auth в URL
        assert!(has_url_credentials(&format!(
            "http://user:{}@host/path",
            "pw"
        )));
        assert!(has_url_credentials("https://tok@host"));
        assert!(!has_url_credentials("https://example.com/path"));
        assert!(!has_url_credentials("просто текст"));
        // Обычные запросы — НЕ секреты.
        assert!(!looks_secretish("как приготовить борщ"));
        assert!(!looks_secretish("rust async tokio best practices 2026"));
        assert!(!looks_secretish(
            "https://example.com/some/long/path/that/is/words"
        ));
    }

    #[test]
    fn search_args_reject_unknown_field() {
        assert!(serde_json::from_str::<SearchArgs>(r#"{"query":"x","bogus":1}"#).is_err());
        assert!(serde_json::from_str::<FetchArgs>(r#"{"url":"http://x","bogus":1}"#).is_err());
        assert!(serde_json::from_str::<SearchArgs>(r#"{"query":"x"}"#).is_ok());
    }

    /// LIVE: реальная модель на риге исследует веб через `web.search` (мета-поиск SearXNG на VPS). Полный
    /// стек вживую: модель → tool-call web.search → GuardedClient(Web, allowlist) → SearXNG → результаты
    /// (фенсятся циклом) → финальный ответ. Запуск:
    /// `NEXUS_LIVE_CHAT=1 cargo test -p nexus-core --lib agent::web_tools::tests::live_agent_web -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live web (нужны SearXNG :8888 + tool-capable модель :8080: NEXUS_LIVE_CHAT=1, NEXUS_LIVE_SEARX_URL default http://89.127.211.153:8888)"]
    async fn live_agent_web_search_on_rig() {
        use crate::agent::event::AgentEvent;
        use crate::agent::session::{run_agent_session, AgentEventForwarder, SessionSpec};
        use crate::ai::tools::OpenAiToolProvider;
        use crate::db::Database;
        use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient};
        use std::sync::atomic::AtomicBool;
        use std::sync::Mutex;
        use std::time::Duration;
        use tempfile::TempDir;

        if std::env::var("NEXUS_LIVE_CHAT").ok().as_deref() != Some("1") {
            eprintln!("SKIP: NEXUS_LIVE_CHAT!=1");
            return;
        }
        let chat_url = std::env::var("NEXUS_LIVE_CHAT_URL")
            .unwrap_or_else(|_| "http://192.168.0.31:8080".into());
        let model =
            std::env::var("NEXUS_LIVE_CHAT_MODEL").unwrap_or_else(|_| "qwen36-mtp.gguf".into());
        let searx_url = std::env::var("NEXUS_LIVE_SEARX_URL")
            .unwrap_or_else(|_| "http://89.127.211.153:8888".into());
        let searx_host = reqwest::Url::parse(&searx_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .expect("searx host");

        // Политика: Chat (риг, local-first) + Web (SearXNG, allowlist публичного хоста).
        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        policy.set_feature_enabled(EgressFeature::Chat, true);
        policy.set_feature_enabled(EgressFeature::Web, true);
        policy.set_scoped_allowlist("web", [searx_host]);
        let audit = Arc::new(EgressAudit::default());
        let gc = GuardedClient::for_chat(policy, audit, Duration::from_secs(25)).unwrap();
        let provider: Arc<dyn crate::ai::tools::ToolCapableProvider> = Arc::new(
            OpenAiToolProvider::new(&gc, EgressFeature::Chat, &chat_url, &model, Some(0.2)),
        );
        let web = WebToolsConfig {
            client: gc.clone(),
            searxng_url: Some(searx_url),
        };

        #[derive(Default)]
        struct Collector(Mutex<Vec<AgentEvent>>);
        impl AgentEventForwarder for Collector {
            fn forward(&self, ev: &AgentEvent) {
                self.0.lock().unwrap().push(ev.clone());
            }
        }
        let fwd = Arc::new(Collector::default());

        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("t.db")).await.unwrap();
        let spec = SessionSpec {
            run_id: 1,
            task:
                "Узнай через инструмент web.search, какая столица у Франции, и дай короткий ответ."
                    .into(),
            autonomy: None,
            actuator_enabled: false,
            overwrite_threshold: 100,
            blast_cap: 10,
            context_window: Some(32768),
            canon_root: dir.path().to_path_buf(),
            skills_learning_enabled: false,
        };
        let paused = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let decision: Arc<dyn crate::actuator::DecisionSource> =
            Arc::new(crate::actuator::PolicyDefault);
        let outcome = run_agent_session(
            &spec,
            provider.as_ref(),
            None,
            None,
            Some(&web),
            decision,
            db.writer(),
            db.reader(),
            &paused,
            &cancel,
            fwd.clone(),
            None,
            None,
        )
        .await;
        eprintln!("LIVE web outcome: {outcome:?}");
        let evs = fwd.0.lock().unwrap();
        for e in evs.iter() {
            eprintln!("  ev: {e:?}");
        }
        let did_search = evs
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCall { kind, .. } if kind == "web.search"));
        let got_final = evs.iter().any(|e| matches!(e, AgentEvent::Final(_)));
        assert!(did_search, "модель должна была вызвать web.search");
        assert!(got_final, "прогон дошёл до финального ответа");
    }
}
