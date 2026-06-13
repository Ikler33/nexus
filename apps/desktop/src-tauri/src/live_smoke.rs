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
            comments_url: None,
        },
        NewsEntry {
            source_id: "smoke".into(),
            url: "https://example.com/b".into(),
            title: "Anthropic ships persistent memory for Claude teams".into(),
            published_at: 0,
            excerpt: "Persistent memory lets the assistant recall project context across sessions."
                .into(),
            comments_url: None,
        },
        NewsEntry {
            source_id: "smoke".into(),
            url: "https://example.com/c".into(),
            title: "Запеканка из кабачков: простой рецепт на ужин".into(),
            published_at: 0,
            excerpt: "Кабачки, сыр и зелень — готово за полчаса.".into(),
            comments_url: None,
        },
    ];

    let report = evaluate_entries(&chat, &entries, false, &cancel(), &|_| {}).await;
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

/// N4 (RAG по чат-сессиям) — ЖИВАЯ сквозная проверка «второго мозга»: в ПРОШЛОЙ сессии пользователь
/// зафиксировал необычный факт; в НОВОЙ сессии спрашивает о нём перефразированно → реальный bge-m3
/// достаёт ту сессию из `chat_vectors`, врезка (`build_memory_block`) уходит в промпт, живая gemma
/// вспоминает факт. КОНТРОЛЬ: без памяти модель факт знать не может (иначе тест не дискриминирует).
/// Это доказывает, что фича работает на настоящих моделях, а не только на моках.
#[tokio::test]
#[ignore = "нужны LLM-сервер + bge-m3 (NEXUS_CHAT_URL/NEXUS_EMBED_URL, default 192.168.0.31)"]
async fn live_chat_memory_recall_end_to_end() {
    use crate::ai::{
        build_chat_messages, build_memory_block, prepend_memory_block, EmbeddingProvider,
    };
    use crate::chat_log::{log_exchange, search_memory};
    use crate::db::Database;
    use crate::vector::VectorIndex;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path().join(".nexus/nexus.db"))
        .await
        .unwrap();
    let vectors = VectorIndex::open(dir.path().join("chat_vectors.usearch"), 1024).unwrap();
    let emb = crate::ai::live_test_embedder();
    let chat = provider(&chat_url(), "gemma-4-26B-A4B-it-qat");

    // Прошлая сессия A: факт, который модель не выдумает («Гелиодор» — редкий минерал, не дефолт).
    let fact_q = "Зафиксируй: кодовое имя следующего релиза нашего проекта — «Гелиодор».";
    let fact_a = "Принято, релиз называется «Гелиодор».";
    let a = log_exchange(db.writer(), None, fact_q, fact_a, None)
        .await
        .unwrap();
    // Сессия-шум B (не про релиз) — поиск должен быть нетривиальным, не «единственный кандидат».
    let noise_q = "Какой соус к пасте карбонара?";
    let noise_a = "Классическая карбонара — на яйце и пекорино, без сливок.";
    let b = log_exchange(db.writer(), None, noise_q, noise_a, None)
        .await
        .unwrap();
    // Индексируем все сообщения реальными bge-m3 эмбеддингами (ключ usearch = id сообщения).
    for (id, text) in [
        (a.user_msg_id, fact_q),
        (a.assistant_msg_id, fact_a),
        (b.user_msg_id, noise_q),
        (b.assistant_msg_id, noise_a),
    ] {
        let v = emb.embed_documents(&[text]).await.unwrap();
        vectors.upsert(id as u64, &v[0]).unwrap();
    }

    // НОВАЯ сессия: вопрос перефразирован (проверяем СЕМАНТИКУ, не совпадение слов).
    let question = "Напомни, как мы назвали наш следующий релиз?";

    // КОНТРОЛЬ: без памяти модель факт знать не может.
    let mut ctrl = String::new();
    chat.stream_chat(
        &build_chat_messages(question),
        &mut |t| ctrl.push_str(&t),
        &cancel(),
    )
    .await
    .unwrap();
    println!("[контроль, без памяти] {ctrl}");
    assert!(
        !ctrl.to_lowercase().contains("гелиодор"),
        "контроль: без памяти модель не должна знать факт"
    );

    // С ПАМЯТЬЮ: search_memory достаёт сессию A (мы в новой сессии → exclude None).
    let hits = search_memory(db.reader(), &vectors, &emb, question, 3, None, 280)
        .await
        .unwrap();
    println!(
        "[память] {} фрагм.: {:?}",
        hits.len(),
        hits.iter()
            .map(|h| (h.session_title.as_str(), h.snippet.as_str()))
            .collect::<Vec<_>>()
    );
    assert!(!hits.is_empty(), "память нашла прошлую сессию");
    assert_eq!(
        hits[0].session_id, a.session_id,
        "ближайший фрагмент — из сессии про релиз, не из шумовой"
    );

    // Врезка памяти в промпт + ответ реальной модели.
    let snippets: Vec<(String, String)> = hits
        .iter()
        .map(|h| {
            let who = if h.role == "user" {
                "вы"
            } else {
                "ассистент"
            };
            (
                format!("Диалог «{}» ({who})", h.session_title),
                h.snippet.clone(),
            )
        })
        .collect();
    let mut msgs = build_chat_messages(question);
    prepend_memory_block(
        &mut msgs,
        build_memory_block(&snippets, &injection_marker()),
    );
    let mut ans = String::new();
    chat.stream_chat(&msgs, &mut |t| ans.push_str(&t), &cancel())
        .await
        .unwrap();
    println!("[с памятью] {ans}");
    assert!(
        ans.to_lowercase().contains("гелиодор"),
        "с памятью модель должна вспомнить «Гелиодор»"
    );
}
