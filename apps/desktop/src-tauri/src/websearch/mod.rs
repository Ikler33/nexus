//! Web-агент: поиск через self-hosted **SearXNG** (egress срез 4, W1–W4). Web-класс фичи
//! `EgressFeature::Web`: `allow_private=false`, DNS-rebinding-гард обязателен, consent = сохранённый
//! URL SearXNG (`websearch.json` в OS config-dir) → авто-allowlist скоупа "web" (W2).
//!
//! - [`config`]: consent-конфиг (URL SearXNG = единственная истина consent, как `news.json`).
//! - [`search`]: SearXNG JSON-клиент через guarded-фетч (resolve→гард→пин→cap), W3-лимиты,
//!   W4 `scan_secrets` исходящего запроса ДО отправки.
//!
//! Сам agent-loop (LLM решает «нужен интернет» → поиск → ответ с цитатами) — срез W-2.

pub mod agent;
pub mod config;
pub mod search;

pub use config::WebSearchConfig;
pub use search::{SearchError, SearchResult, Searcher, WebSearcher};
