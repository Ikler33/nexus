//! Лента новостей — NF-1 (спека `docs/specs/news-feed.md`, решения владельца D1–D7): типы,
//! реестр источников v1 и **этап 1** двухэтапного фильтра (keyword, без LLM, AC-NF-2).
//! Парсинг фидов (RSS 2.0 / Atom / HF daily_papers / HN Algolia → [`NewsEntry`]) — [`mod@parse`].
//!
//! Дальше по нарезке: NF-2 — LLM-этап (RU-заголовок/резюме/темы + сводка дня), NF-3 — персист
//! `news_items` + scheduled-kind, NF-4 — сетевой слой (`EgressFeature::NewsFeed`, лимиты W3,
//! DNS-rebinding-гард), NF-5 — страница UI. Контент фидов НЕДОВЕРЕННЫЙ: в LLM-промпты он пойдёт
//! только между injection-маркерами (AC-SEC-7-паттерн), а здесь — никогда не интерпретируется.

mod article;
mod config;
mod fetch;
mod llm;
mod parse;
mod run;
mod store;

pub use article::{extract_paragraphs, summarize_article, translate_article};
pub use config::{load as load_news_config, save as save_news_config, NewsConfig};
pub use fetch::{
    check_resolved_ips, read_body_capped, GuardedNewsFetcher, Resolver, SystemResolver,
    FEED_BODY_CAP,
};
pub use llm::{daily_digest, evaluate_entries, EvalReport, EvaluatedEntry};
pub use parse::parse_feed;
pub use run::{run_news_pipeline, FeedFetcher, NewsFeedHandler, KIND_NEWSFEED, LLM_RUN_CAP};
pub use store::{
    filter_new_urls, get_body, get_item, insert_items, latest_run, list_items, list_topics,
    mark_read, record_run, retention_gc, set_body, NewRow, NewsItem, NewsRun, RETENTION_DAYS,
};

use thiserror::Error;

/// Максимальная длина выжимки записи (символы): хватает keyword-фильтру и LLM-этапу
/// (~200 токенов на вход по концепту), не тащим полные статьи в БД/промпты.
pub const EXCERPT_MAX_CHARS: usize = 500;

/// Ошибки разбора фида. Битый фид НЕ валит прогон: источник пропускается с видимой ошибкой
/// в сводке прогона (AC-NF-1; агрегирует NF-3).
#[derive(Debug, Error)]
pub enum NewsError {
    #[error("фид не разобран: {0}")]
    Parse(String),
}

/// Нормализованная запись фида (AC-NF-1) — общий вход keyword- и LLM-этапов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsEntry {
    pub source_id: String,
    pub url: String,
    pub title: String,
    /// Unix-секунды публикации; `0` — у записи нет даты (сортируется как самая старая).
    pub published_at: i64,
    /// Текстовая выжимка без HTML (≤ [`EXCERPT_MAX_CHARS`]).
    pub excerpt: String,
}

/// Формат фида источника.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedKind {
    Rss,
    Atom,
    /// `huggingface.co/api/daily_papers` (JSON-массив; кураторская выжимка arxiv).
    HfDailyPapers,
    /// HN Algolia search API (`query` подставляется из ключевых слов на сетевом слое, NF-4).
    HnAlgolia,
}

/// Источник ленты (реестр v1 — спека D1; фиды прозвонены вживую 2026-06-10).
#[derive(Debug, Clone, Copy)]
pub struct Source {
    pub id: &'static str,
    pub title: &'static str,
    /// URL фида; для [`FeedKind::HnAlgolia`] — шаблон с `{query}` (заполняет NF-4 из ключей).
    pub url: &'static str,
    pub kind: FeedKind,
    /// Высокопоточный источник → keyword-фильтр на входе (AC-NF-2, D2);
    /// малопоточные (блоги вендоров/релизы) идут в LLM-этап целиком.
    pub high_volume: bool,
    /// arxiv-категории выключены из коробки (D1: шум; HF Papers — их кураторская выжимка).
    pub default_enabled: bool,
    /// Русскоязычный источник: LLM-этап не «переводит» (резюме и так пишется по-русски).
    pub lang_ru: bool,
}

/// Реестр источников v1 (спека D1). Anthropic без RSS → покрывается HN-ключами (`anthropic`,
/// `claude` в пресете) + Simon Willison; HTML-мост — v2 (вне скоупа).
pub const SOURCES_V1: &[Source] = &[
    Source {
        id: "openai",
        title: "OpenAI",
        url: "https://openai.com/news/rss.xml",
        kind: FeedKind::Rss,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "deepmind",
        title: "Google DeepMind",
        url: "https://deepmind.google/blog/rss.xml",
        kind: FeedKind::Rss,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "google-ai",
        title: "Google AI",
        url: "https://blog.google/technology/ai/rss/",
        kind: FeedKind::Rss,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "mistral",
        title: "Mistral",
        url: "https://mistral.ai/rss.xml",
        kind: FeedKind::Rss,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "qwen",
        title: "Qwen",
        url: "https://qwenlm.github.io/blog/index.xml",
        kind: FeedKind::Rss,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "hf-blog",
        title: "Hugging Face Blog",
        url: "https://huggingface.co/blog/feed.xml",
        kind: FeedKind::Rss,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "hf-papers",
        title: "HF Daily Papers",
        url: "https://huggingface.co/api/daily_papers",
        kind: FeedKind::HfDailyPapers,
        high_volume: true,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "willison",
        title: "Simon Willison",
        url: "https://simonwillison.net/atom/everything/",
        kind: FeedKind::Atom,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "raschka",
        title: "Sebastian Raschka",
        url: "https://magazine.sebastianraschka.com/feed",
        kind: FeedKind::Rss,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "gradient",
        title: "The Gradient",
        url: "https://thegradient.pub/rss/",
        kind: FeedKind::Rss,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "lastweekinai",
        title: "Last Week in AI",
        url: "https://lastweekin.ai/feed",
        kind: FeedKind::Rss,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "llama-cpp",
        title: "llama.cpp releases",
        url: "https://github.com/ggml-org/llama.cpp/releases.atom",
        kind: FeedKind::Atom,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "ollama",
        title: "ollama releases",
        url: "https://github.com/ollama/ollama/releases.atom",
        kind: FeedKind::Atom,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "vllm",
        title: "vLLM releases",
        url: "https://github.com/vllm-project/vllm/releases.atom",
        kind: FeedKind::Atom,
        high_volume: false,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "hn",
        title: "HackerNews",
        url: "https://hn.algolia.com/api/v1/search_by_date?tags=story&query={query}",
        kind: FeedKind::HnAlgolia,
        high_volume: true,
        default_enabled: true,
        lang_ru: false,
    },
    Source {
        id: "habr-ai",
        title: "Хабр · Искусственный интеллект",
        url: "https://habr.com/ru/rss/hub/artificial_intelligence/all/",
        kind: FeedKind::Rss,
        high_volume: true,
        default_enabled: true,
        lang_ru: true,
    },
    Source {
        id: "arxiv-cs-ai",
        title: "arxiv cs.AI",
        url: "https://rss.arxiv.org/rss/cs.AI",
        kind: FeedKind::Rss,
        high_volume: true,
        default_enabled: false,
        lang_ru: false,
    },
    Source {
        id: "arxiv-cs-lg",
        title: "arxiv cs.LG",
        url: "https://rss.arxiv.org/rss/cs.LG",
        kind: FeedKind::Rss,
        high_volume: true,
        default_enabled: false,
        lang_ru: false,
    },
    Source {
        id: "arxiv-cs-cl",
        title: "arxiv cs.CL",
        url: "https://rss.arxiv.org/rss/cs.CL",
        kind: FeedKind::Rss,
        high_volume: true,
        default_enabled: false,
        lang_ru: false,
    },
];

/// Пресет ключевых слов этапа 1 (D2; редактируемый список — в `news.json`-конфиге, NF-3).
pub const DEFAULT_KEYWORDS: &[&str] = &[
    "claude",
    "anthropic",
    "gpt",
    "openai",
    "gemini",
    "qwen",
    "llama",
    "mistral",
    "llm",
    "rag",
    "embedding",
    "agent",
    "mcp",
    "inference",
    "quantization",
    "fine-tuning",
    "vllm",
    "llama.cpp",
    "ollama",
    "transformer",
    "reasoning",
];

/// Хосты активных источников — для "news"-скоупа allowlist (consent = включение фичи, NF-4).
/// HN-шаблон резолвится подстановкой плейсхолдера (host от query не зависит).
pub fn news_hosts(cfg: &NewsConfig) -> Vec<String> {
    cfg.active_sources()
        .iter()
        .filter_map(|s| {
            let url = s.url.replace("{query}", "q");
            reqwest::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
        })
        .collect()
}

/// Синхронизирует политику эгресса с конфигом ленты (NF-4, AC-NF-7): тоггл фичи = consent;
/// хосты активных источников → "news"-скоуп allowlist (выключена → скоуп пуст — fail-closed).
/// Единственная истина — `news.json`; вызывается на старте приложения и из `set_news_config`.
pub fn sync_egress_policy(policy: &crate::net::EgressPolicy, cfg: &NewsConfig) {
    policy.set_feature_enabled(crate::net::EgressFeature::NewsFeed, cfg.enabled);
    let hosts = if cfg.enabled {
        news_hosts(cfg)
    } else {
        Vec::new()
    };
    policy.set_scoped_allowlist("news", hosts);
}

/// Этап 1 фильтра (AC-NF-2): для `high_volume`-источников оставляет записи, у которых
/// title+excerpt (unicode lowercase) содержит хотя бы один ключ; остальные источники проходят
/// целиком (редкий пост вендора не теряем из-за неудачного слова, D2). Пустые ключи при
/// `high_volume` → ПУСТО (fail-closed к LLM-бюджету; предупреждение — на вызывающем, NF-3).
pub fn keyword_filter(
    entries: Vec<NewsEntry>,
    source: &Source,
    keywords: &[String],
) -> Vec<NewsEntry> {
    if !source.high_volume {
        return entries;
    }
    if keywords.is_empty() {
        return Vec::new();
    }
    let keys: Vec<String> = keywords
        .iter()
        .map(|k| k.trim().to_lowercase())
        .filter(|k| !k.is_empty())
        .collect();
    entries
        .into_iter()
        .filter(|e| {
            let hay = format!("{} {}", e.title, e.excerpt).to_lowercase();
            keys.iter().any(|k| hay.contains(k))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, excerpt: &str) -> NewsEntry {
        NewsEntry {
            source_id: "test".into(),
            url: "https://example.com/a".into(),
            title: title.into(),
            published_at: 1_750_000_000,
            excerpt: excerpt.into(),
        }
    }

    fn src(high_volume: bool) -> Source {
        Source {
            id: "test",
            title: "Test",
            url: "https://example.com/feed",
            kind: FeedKind::Rss,
            high_volume,
            default_enabled: true,
            lang_ru: false,
        }
    }

    /// AC-NF-2: high_volume фильтруется по ключам (unicode case-insensitive, title+excerpt);
    /// малопоточный источник проходит целиком; пустые ключи → fail-closed (пусто).
    #[test]
    fn keyword_filter_per_spec() {
        let entries = vec![
            entry("Claude 5 released", ""), // ключ в title (регистр)
            entry("Weekly digest", "обзор квантизации моделей"), // RU-ключ в excerpt
            entry("Gardening tips", "tomatoes"), // мимо
        ];
        let keys = vec!["claude".to_string(), "квантизаци".to_string()];

        let kept = keyword_filter(entries.clone(), &src(true), &keys);
        assert_eq!(kept.len(), 2, "оставлены только записи с ключами");
        assert!(kept.iter().all(|e| e.title != "Gardening tips"));

        // Малопоточный источник — без фильтра (редкий пост вендора не теряем, D2).
        assert_eq!(keyword_filter(entries.clone(), &src(false), &keys).len(), 3);

        // Пустые ключи при high_volume → пусто (fail-closed к LLM-бюджету), не «всё подряд».
        assert!(keyword_filter(entries, &src(true), &[]).is_empty());
    }

    /// Реестр v1 согласован со спекой: id уникальны, HN — шаблон {query}, arxiv выключен
    /// из коробки, Хабр помечен русскоязычным, высокопоточные — ровно те, что в D1/D2.
    #[test]
    fn sources_registry_matches_spec() {
        let mut ids: Vec<_> = SOURCES_V1.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "id источников уникальны");

        let hn = SOURCES_V1.iter().find(|s| s.id == "hn").unwrap();
        assert!(hn.url.contains("{query}") && hn.high_volume);

        assert!(
            SOURCES_V1
                .iter()
                .filter(|s| s.id.starts_with("arxiv"))
                .all(|s| !s.default_enabled && s.high_volume),
            "arxiv: выключен по умолчанию и высокопоточный (D1)"
        );
        assert!(
            SOURCES_V1
                .iter()
                .find(|s| s.id == "habr-ai")
                .unwrap()
                .lang_ru
        );

        let hv: Vec<_> = SOURCES_V1
            .iter()
            .filter(|s| s.high_volume)
            .map(|s| s.id)
            .collect();
        assert_eq!(
            hv,
            vec![
                "hf-papers",
                "hn",
                "habr-ai",
                "arxiv-cs-ai",
                "arxiv-cs-lg",
                "arxiv-cs-cl"
            ]
        );

        assert!(!DEFAULT_KEYWORDS.is_empty());
        assert!(
            DEFAULT_KEYWORDS.contains(&"anthropic"),
            "Anthropic покрывается HN-ключами (D1)"
        );
    }
}
