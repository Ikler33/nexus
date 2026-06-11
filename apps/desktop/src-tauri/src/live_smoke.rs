//! Live-smoke LLM-этапов против прод-серверов (все тесты игнорируются по умолчанию, в CI не исполняются):
//! гоняет ПРОДАКШЕН-промпты и парсеры стадий на живых моделях — то, что юнитами с моками не
//! проверяется (реальная модель может отвечать вне контракта). Покрывает: новостной LLM-этап
//! (RU-резюме/темы + сводка дня) и web-агента целиком (план → SearXNG → ответ с источниками),
//! включая декларацию «веб не нужен». Чат-стрим/эмбеддер/eval — соседние live-тесты в `ai::chat`,
//! `ai::embedder`, `eval::live_tests`.
//!
//! Дефолтные хосты — текущие прод-сервера (LLM 192.168.0.31, SearXNG на VPS); переопределяются env:
//!   `NEXUS_CHAT_URL`  (default `http://192.168.0.31:8080`, Gemma 26B — ответы)
//!   `NEXUS_FAST_URL`  (default `http://192.168.0.31:8084`, Gemma 12B — планировщик/примитивы)
//!   `NEXUS_SEARX_URL` (default `http://89.127.211.153:8888`, SearXNG)
//! Запуск: `cargo test live_ -- --ignored --nocapture` (или поимённо).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::ai::{
    build_web_answer_messages, injection_marker, ChatMessage, ChatProvider, OpenAiChatProvider,
};
use crate::net::{EgressFeature, EgressPolicy, GuardedClient};
use crate::news::{daily_digest, evaluate_entries, NewsEntry, SystemResolver};
use crate::websearch::config::sync_egress_policy;
use crate::websearch::{agent, WebSearchConfig, WebSearcher};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn chat_url() -> String {
    env_or("NEXUS_CHAT_URL", "http://192.168.0.31:8080")
}

fn fast_url() -> String {
    env_or("NEXUS_FAST_URL", "http://192.168.0.31:8084")
}

fn searx_url() -> String {
    env_or("NEXUS_SEARX_URL", "http://89.127.211.153:8888")
}

/// Прод-провайдер на живой сервер. llama.cpp с одной моделью игнорирует имя — оно тут для логов.
fn provider(url: &str, model: &str) -> Arc<dyn ChatProvider> {
    Arc::new(OpenAiChatProvider::new(
        &GuardedClient::unchecked(),
        EgressFeature::Chat,
        url,
        model,
        Some(0.0),
    ))
}

fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn has_cyrillic(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё'))
}

/// Поисковик как в проде (`commands::chat`): реальная политика (consent через `sync_egress_policy`,
/// W2) + системный резолвер с DNS-гардом — smoke проверяет и wiring эгресса, не только HTTP.
fn live_searcher() -> WebSearcher {
    let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
    let cfg = WebSearchConfig {
        enabled: true,
        url: searx_url(),
    };
    sync_egress_policy(&policy, &cfg);
    WebSearcher::new(
        policy,
        Arc::new(crate::net::EgressAudit::default()),
        Arc::new(SystemResolver),
        searx_url(),
    )
}

/// Новостной LLM-этап на быстрой модели (прод-путь: news идёт на `chat_util`): фильтрация
/// релевантности + RU-резюме/тема (AC-NF-3), затем сводка дня (AC-NF-10) по оценённому.
#[tokio::test]
#[ignore = "нужен живой LLM-сервер (NEXUS_FAST_URL, default 192.168.0.31:8084)"]
async fn live_news_eval_and_daily_digest() {
    let chat = provider(&fast_url(), "gemma-4-12B-it-qat");
    let entries = vec![
        NewsEntry {
            source_id: "smoke".into(),
            url: "https://example.com/a".into(),
            title: "OpenAI releases new reasoning model with tool use".into(),
            published_at: 0,
            excerpt:
                "The model improves multi-step reasoning and agentic tool use across benchmarks."
                    .into(),
        },
        NewsEntry {
            source_id: "smoke".into(),
            url: "https://example.com/b".into(),
            title: "Anthropic ships persistent memory for Claude teams".into(),
            published_at: 0,
            excerpt: "Persistent memory lets the assistant recall project context across sessions."
                .into(),
        },
        NewsEntry {
            source_id: "smoke".into(),
            url: "https://example.com/c".into(),
            title: "Запеканка из кабачков: простой рецепт на ужин".into(),
            published_at: 0,
            excerpt: "Кабачки, сыр и зелень — готово за полчаса.".into(),
        },
    ];

    let report = evaluate_entries(&chat, &entries, false, &cancel()).await;
    println!(
        "news eval: items={} irrelevant={} failed={}",
        report.items.len(),
        report.irrelevant,
        report.failed
    );
    for it in &report.items {
        println!("  [{}] {} — {}", it.topic, it.title_ru, it.summary_ru);
    }
    assert_eq!(report.failed, 0, "модель обязана держать JSON-контракт");
    assert!(
        report.items.len() >= 2,
        "обе AI-новости должны пройти фильтр"
    );
    assert!(report.irrelevant >= 1, "рецепт должен отсеяться");
    for it in &report.items {
        assert!(
            has_cyrillic(&it.title_ru) && has_cyrillic(&it.summary_ru),
            "title_ru/summary_ru — по-русски: {it:?}"
        );
    }

    let digest = daily_digest(&chat, &report.items, &cancel()).await.unwrap();
    println!("daily digest:\n{digest}");
    assert!(!digest.is_empty());
    assert!(has_cyrillic(&digest), "сводка дня — по-русски");
}

/// Web-агент целиком, как в `chat_rag(web=true)`: план на быстрой модели → реальный SearXNG (VPS,
/// W2-консент/W3-лимит/W4-скан/DNS-гард) → ответ большой модели с источниками-маркерами.
#[tokio::test]
#[ignore = "нужны LLM-сервер и SearXNG (NEXUS_FAST_URL/NEXUS_CHAT_URL/NEXUS_SEARX_URL)"]
async fn live_web_agent_plans_searches_answers() {
    let planner = provider(&fast_url(), "gemma-4-12B-it-qat");
    let searcher = live_searcher();

    let question = "Какая сейчас последняя стабильная версия Python и когда она вышла?";
    let outcome = agent::run(planner.as_ref(), &searcher, question, &cancel())
        .await
        .expect("план+поиск без ошибок");
    let query = outcome.query.expect("свежий факт → модель обязана искать");
    println!("web agent query: {query} (fresh={})", outcome.fresh);
    assert!(
        outcome.fresh,
        "вопрос про «последнюю версию» — план обязан пометить FRESH (time_range)"
    );
    assert!(!outcome.results.is_empty(), "SearXNG дал результаты");
    for r in outcome.results.iter().take(3) {
        println!("  {} — {}", r.title, r.url);
    }

    // Ответ — на большой модели (прод-путь): источники как недоверенный контекст в маркерах.
    let triples: Vec<(String, String, String)> = outcome
        .results
        .iter()
        .map(|r| (r.title.clone(), r.url.clone(), r.snippet.clone()))
        .collect();
    let messages = build_web_answer_messages(question, &triples, &injection_marker());
    let answerer = provider(&chat_url(), "gemma-4-26B-A4B-it-qat");
    let mut sink = |_: String| {};
    let answer = answerer
        .stream_chat(&messages, &mut sink, &cancel())
        .await
        .unwrap();
    println!("web answer:\n{answer}");
    assert!(!answer.trim().is_empty());
    assert!(has_cyrillic(&answer), "ответ пользователю — по-русски");
}

/// Decide-этап: на вопрос без потребности в интернете план обязан вернуть «веб не нужен»
/// (деградация к общему чату, поиск не зовётся).
#[tokio::test]
#[ignore = "нужен живой LLM-сервер (NEXUS_FAST_URL, default 192.168.0.31:8084)"]
async fn live_web_agent_declines_offline_question() {
    let planner = provider(&fast_url(), "gemma-4-12B-it-qat");
    let searcher = live_searcher();
    let outcome = agent::run(
        planner.as_ref(),
        &searcher,
        "Сколько будет 17 умножить на 3?",
        &cancel(),
    )
    .await
    .unwrap();
    assert!(
        outcome.query.is_none(),
        "арифметика не требует веба, модель запланировала: {:?}",
        outcome.query
    );
}

/// Прямой smoke чат-стрима большой модели (как `live_chat_streams_tokens`, но на прод-URL .31):
/// токены капают, ответ осмысленный.
#[tokio::test]
#[ignore = "нужен живой LLM-сервер (NEXUS_CHAT_URL, default 192.168.0.31:8080)"]
async fn live_gemma26_chat_answers() {
    let chat = provider(&chat_url(), "gemma-4-26B-A4B-it-qat");
    let mut tokens = 0usize;
    let mut on_token = |_: String| tokens += 1;
    let full = chat
        .stream_chat(
            &[ChatMessage::user("Ответь одним словом: столица Японии?")],
            &mut on_token,
            &cancel(),
        )
        .await
        .unwrap();
    println!("gemma26: {} токенов, ответ: {full}", tokens);
    assert!(tokens > 0);
    assert!(full.to_lowercase().contains("токио") || full.to_lowercase().contains("tokyo"));
}
