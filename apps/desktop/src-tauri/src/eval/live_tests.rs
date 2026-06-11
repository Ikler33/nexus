//! Live/bench-тесты eval-харнесса (все игнорируются по умолчанию): нужен живой bge-m3 / реальный vault.
//! Вынесены из `mod.rs` (ночь 2026-06-11): их тела принципиально не исполняются в CI (нужен
//! сервер) и давили метрику покрытия модуля; гейт покрытия меряет `eval/mod.rs`
//! (`scripts/check-coverage.mjs`). Запуск как раньше:
//!   `NEXUS_EMBED_URL=http://192.168.0.31:8083 cargo test <имя> -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use super::tests::{golden_hash, EvalFixture};
use super::*;
use crate::ai::{EmbeddingProvider, OpenAiEmbedder};
use crate::db::Database;
use crate::indexer::Indexer;
use crate::search::{self, SearchOptions};
use crate::vector::VectorIndex;

/// URL живого embedding-сервера: env `NEXUS_EMBED_URL` или `embedding_server` из baseline (сервер
/// мог переехать — напр. с `127.0.0.1` на LAN-хост, при этом модель/dim те же).
fn live_embed_url(cond: &Conditions) -> String {
    std::env::var("NEXUS_EMBED_URL").unwrap_or_else(|_| cond.embedding_server.clone())
}

/// Обёртка вокруг живого эмбеддера, записывающая каждую пару (текст → вектор) для регенерации фикстуры.
struct RecordingEmbedder {
    inner: OpenAiEmbedder,
    docs: Mutex<BTreeMap<String, Vec<f32>>>,
    queries: Mutex<BTreeMap<String, Vec<f32>>>,
}
#[async_trait::async_trait]
impl EmbeddingProvider for RecordingEmbedder {
    async fn embed_documents(&self, texts: &[&str]) -> crate::ai::AiResult<Vec<Vec<f32>>> {
        let vecs = self.inner.embed_documents(texts).await?;
        let mut g = self.docs.lock().unwrap();
        for (t, v) in texts.iter().zip(&vecs) {
            g.insert((*t).to_string(), v.clone());
        }
        Ok(vecs)
    }
    async fn embed_query(&self, text: &str) -> crate::ai::AiResult<Vec<f32>> {
        let v = self.inner.embed_query(text).await?;
        self.queries
            .lock()
            .unwrap()
            .insert(text.to_string(), v.clone());
        Ok(v)
    }
    fn dim(&self) -> usize {
        self.inner.dim()
    }
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
}

/// Регенерация фикстуры (ignored-тест): прогон golden через ЖИВОЙ bge-m3 → запись реальных векторов в
/// `eval/fixture_bge_m3.json`. Пишет ТОЛЬКО если метрики ≥ baseline (не фиксируем плохой прогон).
/// `NEXUS_EMBED_URL=http://192.168.0.31:8083 cargo test regen_eval_fixture -- --ignored --nocapture`
#[tokio::test]
#[ignore = "разовая регенерация: нужен живой bge-m3 (NEXUS_EMBED_URL или baseline server)"]
async fn regen_eval_fixture() {
    use crate::ai::default_prefixes;
    let golden = load_golden();
    let baseline = load_baseline();
    let cond = &baseline.conditions;
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();

    let real = OpenAiEmbedder::new(
        &crate::net::GuardedClient::unchecked(),
        crate::net::EgressFeature::Embed,
        &live_embed_url(cond),
        &cond.embedding_model,
        cond.embedding_dim,
        default_prefixes(&cond.embedding_model),
    );
    let rec = Arc::new(RecordingEmbedder {
        inner: real,
        docs: Mutex::new(BTreeMap::new()),
        queries: Mutex::new(BTreeMap::new()),
    });
    let embedder: Arc<dyn EmbeddingProvider> = rec.clone();

    let vectors = Arc::new(
        VectorIndex::open(root.join(".nexus/vectors.usearch"), cond.embedding_dim).unwrap(),
    );
    let db = index_corpus(&root, &golden.corpus, embedder.clone(), vectors.clone())
        .await
        .unwrap();
    let report = run_eval(
        db.reader(),
        &vectors,
        embedder.as_ref(),
        &golden.cases,
        cond.k,
    )
    .await
    .unwrap();

    assert!(
        report.recall_at_k >= baseline.metrics.recall_at_k
            && report.ndcg_at_k >= baseline.metrics.ndcg_at_k
            && report.mrr >= baseline.metrics.mrr,
        "live прогон НИЖЕ baseline — фикстуру не пишу: r={:.3} ndcg={:.3} mrr={:.3}",
        report.recall_at_k,
        report.ndcg_at_k,
        report.mrr
    );

    let fixture = EvalFixture {
        model: cond.embedding_model.clone(),
        dim: cond.embedding_dim,
        golden_hash: golden_hash(),
        docs: rec.docs.lock().unwrap().clone(),
        queries: rec.queries.lock().unwrap().clone(),
    };
    let json = serde_json::to_string_pretty(&fixture).unwrap();
    std::fs::write("eval/fixture_bge_m3.json", json).unwrap();
    eprintln!(
        "\n=== fixture записана: {} чанков, {} запросов → eval/fixture_bge_m3.json ===\n\
         r@{}={:.3} nDCG@{}={:.3} MRR={:.3}",
        fixture.docs.len(),
        fixture.queries.len(),
        cond.k,
        report.recall_at_k,
        cond.k,
        report.ndcg_at_k,
        report.mrr
    );
}

/// Живой прогон (`cargo test -- --ignored`): печатает отчёт и проверяет, что
/// метрики НЕ ниже baseline (AC-EVAL-2/3). Условия — в выводе (AC-EVAL-4).
#[tokio::test]
#[ignore = "нужен embedding-сервер из baseline.json (AC-EVAL прогон)"]
async fn live_eval_meets_baseline() {
    use crate::ai::default_prefixes;
    let golden = load_golden();
    let baseline = load_baseline();
    let cond = &baseline.conditions;
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();

    // Эмбеддер и k — строго из условий baseline (AC-EVAL-4: прогон в зафиксированных условиях).
    // URL сервера — из env `NEXUS_EMBED_URL` (если задан), иначе из baseline (сервер мог переехать).
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(OpenAiEmbedder::new(
        &crate::net::GuardedClient::unchecked(),
        crate::net::EgressFeature::Embed,
        &live_embed_url(cond),
        &cond.embedding_model,
        cond.embedding_dim,
        default_prefixes(&cond.embedding_model),
    ));
    let vectors = Arc::new(
        VectorIndex::open(root.join(".nexus/vectors.usearch"), cond.embedding_dim).unwrap(),
    );
    let db = index_corpus(&root, &golden.corpus, embedder.clone(), vectors.clone())
        .await
        .unwrap();

    let report = run_eval(
        db.reader(),
        &vectors,
        embedder.as_ref(),
        &golden.cases,
        cond.k,
    )
    .await
    .unwrap();

    eprintln!(
        "\n=== RAG EVAL ({} @ {}, k={}, n={}) ===\nrecall@{}={:.3} nDCG@{}={:.3} MRR={:.3}",
        cond.embedding_model,
        cond.embedding_server,
        report.k,
        report.n_cases,
        report.k,
        report.recall_at_k,
        report.k,
        report.ndcg_at_k,
        report.mrr
    );
    for c in &report.cases {
        eprintln!(
            "  r={:.2} ndcg={:.2} rr={:.2} | {} [{}] -> {:?}",
            c.recall_at_k, c.ndcg_at_k, c.reciprocal_rank, c.query, c.note, c.hits
        );
    }

    assert!(
        report.recall_at_k >= baseline.metrics.recall_at_k,
        "recall@8 {:.3} < baseline {:.3} (AC-EVAL-3)",
        report.recall_at_k,
        baseline.metrics.recall_at_k
    );
    assert!(
        report.ndcg_at_k >= baseline.metrics.ndcg_at_k,
        "nDCG@8 {:.3} < baseline {:.3}",
        report.ndcg_at_k,
        baseline.metrics.ndcg_at_k
    );
    assert!(
        report.mrr >= baseline.metrics.mrr,
        "MRR {:.3} < baseline {:.3}",
        report.mrr,
        baseline.metrics.mrr
    );
}

/// Живой smoke по РЕАЛЬНОМУ vault из env `NEXUS_TEST_VAULT` на bge-m3 :8083. Индексирует vault
/// целиком во ВРЕМЕННЫЕ db+usearch (реальный `.nexus/` не трогаем) и проверяет кросс-язычный
/// гибридный поиск на живом контенте. Тихо выходит, если env не задан.
///
/// `NEXUS_TEST_VAULT=~/Documents/nexus-test-vault \`
/// `  cargo test live_real_vault_smoke -- --ignored --nocapture`
///
/// Контракт — recall@8 (как в baseline), НЕ @5: на крошечном корпусе у BM25 слабый IDF, поэтому
/// стоп-слова запроса («на», «без»…) лексически цепляют неродственные RU-заметки и через RRF
/// поднимают их над семантически верной кросс-язычной заметкой (та находится вектором на ранге ~0,
/// но живёт в одном списке → ниже по RRF). На реальном vault IDF давит стоп-слова. См. BACKLOG.
#[tokio::test]
#[ignore = "нужен реальный vault в NEXUS_TEST_VAULT + bge-m3 :8083"]
async fn live_real_vault_smoke() {
    use crate::ai::default_prefixes;
    let Ok(vault) = std::env::var("NEXUS_TEST_VAULT") else {
        eprintln!("NEXUS_TEST_VAULT не задан — пропуск");
        return;
    };
    // Разворачиваем ведущий ~/ (cargo не делает shell-expansion для env).
    let vault = match vault.strip_prefix("~/") {
        Some(rest) => format!("{}/{}", std::env::var("HOME").unwrap_or_default(), rest),
        None => vault,
    };
    let root = std::path::PathBuf::from(vault);

    let tmp = TempDir::new().unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(OpenAiEmbedder::new(
        &crate::net::GuardedClient::unchecked(),
        crate::net::EgressFeature::Embed,
        &std::env::var("NEXUS_EMBED_URL").unwrap_or_else(|_| "http://192.168.0.31:8083".into()),
        "bge-m3",
        1024,
        default_prefixes("bge-m3"),
    ));
    let vectors = Arc::new(VectorIndex::open(tmp.path().join("vectors.usearch"), 1024).unwrap());
    let db = Database::open(tmp.path().join("nexus.db")).await.unwrap();
    let indexer = Indexer::with_rag(&db, root.clone(), embedder.clone(), vectors.clone(), true);
    indexer.scan_vault().await.unwrap();

    // Self-retrieval (vault-агностично, 2026-06-12): берём выборку заметок vault, запрос — начало
    // их содержимого, ожидаем, что заметка находит САМУ СЕБЯ в топ-8. Не зависит от конкретного
    // контента (прежние хардкод-пробы были под старый личный vault и ложно падали на рабочем).
    let sample: Vec<(String, String)> = db
        .reader()
        .query(|c| {
            let mut stmt = c.prepare(
                "SELECT f.path, ch.content FROM files f \
                 JOIN chunks ch ON ch.file_id = f.id \
                 WHERE f.is_deleted=0 GROUP BY f.id ORDER BY f.path LIMIT 12",
            )?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .unwrap();
    assert!(!sample.is_empty(), "vault пуст или не проиндексировался");

    let mut ok = 0usize;
    let total = sample.len();
    for (path, content) in &sample {
        // Запрос — первые ~120 символов содержимого (заголовок + начало): естественный «вопрос»
        // про заметку, а не дословная копия чанка целиком.
        let query: String = content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(120)
            .collect();
        let hits = search::hybrid_search(
            db.reader(),
            Some(&vectors),
            Some(embedder.as_ref()),
            query,
            SearchOptions {
                limit: 8,
                filter: None,
                center: None,
            },
        )
        .await
        .unwrap();
        let rank = hits.iter().position(|h| &h.path == path);
        ok += usize::from(rank.is_some());
        eprintln!(
            "[{}] {path} → self-rank={rank:?}",
            if rank.is_some() { "OK" } else { "--" }
        );
    }
    // Self-retrieval почти идеален: заметка по своему же тексту обязана быть в топ-8. Порог 80% —
    // запас на дубли/near-duplicate контент рабочих vault.
    let need = (total * 4).div_ceil(5);
    assert!(
        ok >= need,
        "self-retrieval {ok}/{total} ниже порога {need} — индексация/поиск деградировали"
    );
}

/// **Нагрузочный бенчмарк полного пайплайна** (индексация С ЭМБЕДДИНГАМИ) на синтетическом
/// vault — реальные числа AC-PERF: throughput индексации, латентность поиска и графа,
/// проекция времени полной индексации на 50k. Требует живой bge-m3 :8083.
/// Размер задаётся `NEXUS_BENCH_FILES` (по умолчанию 500):
///   `NEXUS_BENCH_FILES=2000 cargo test bench_index_scale -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn bench_index_scale() {
    use crate::ai::default_prefixes;
    use std::time::Instant;

    let n: usize = std::env::var("NEXUS_BENCH_FILES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let target = 50_000usize;

    // 1) Синтетический vault: N заметок (RU+EN тело → реальные чанки) + 3 вики-ссылки на соседей.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let gen0 = Instant::now();
    for i in 0..n {
        let body = format!(
            "# Note {i}\n\n\
             Синтетическая заметка номер {i} для нагрузочного теста. Немного русского текста и \
             some English text про vector search, knowledge base и retrieval augmented generation. \
             Второй параграф: индексация, эмбеддинги, гибридный поиск, граф связей, чанкинг.\n\n\
             Связи: [[Note-{}]] [[Note-{}]] [[Note-{}]]\n\n#bench #note{}\n",
            (i + 1) % n,
            (i + 7) % n,
            (i + 53) % n,
            i % 20,
        );
        std::fs::write(root.join(format!("Note-{i}.md")), body).unwrap();
    }
    let gen_ms = gen0.elapsed().as_millis();

    // 2) Полный пайплайн с живым эмбеддером bge-m3.
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(OpenAiEmbedder::new(
        &crate::net::GuardedClient::unchecked(),
        crate::net::EgressFeature::Embed,
        &std::env::var("NEXUS_EMBED_URL").unwrap_or_else(|_| "http://192.168.0.31:8083".into()),
        "bge-m3",
        1024,
        default_prefixes("bge-m3"),
    ));
    let vectors = Arc::new(VectorIndex::open(root.join(".nexus/vectors.usearch"), 1024).unwrap());
    let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
    let indexer = Indexer::with_rag(&db, root.clone(), embedder.clone(), vectors.clone(), true);

    let idx0 = Instant::now();
    indexer.scan_vault().await.unwrap();
    let idx_s = idx0.elapsed().as_secs_f64();

    let chunks: i64 = db
        .reader()
        .query(|c| c.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0)))
        .await
        .unwrap();
    let files_per_s = n as f64 / idx_s;
    let emb_per_s = chunks as f64 / idx_s;
    let proj_50k_s = target as f64 / files_per_s;

    // 3) Латентность поиска (гибрид + эмбеддинг запроса).
    let q0 = Instant::now();
    let hits = search::hybrid_search(
        db.reader(),
        Some(&vectors),
        Some(embedder.as_ref()),
        "vector search и граф связей".to_string(),
        SearchOptions {
            limit: 8,
            filter: None,
            center: None,
        },
    )
    .await
    .unwrap();
    let search_ms = q0.elapsed().as_millis();

    // 4) Латентность графа (единый топ-2000 + локальный 2-hop).
    let fg0 = Instant::now();
    let full = crate::graph::get_full_graph(db.reader(), 2000)
        .await
        .unwrap();
    let full_ms = fg0.elapsed().as_millis();
    let lg0 = Instant::now();
    let local = crate::graph::get_local_graph(db.reader(), "Note-0.md".to_string(), 2)
        .await
        .unwrap();
    let local_ms = lg0.elapsed().as_millis();

    eprintln!("\n=== NEXUS bench: полный пайплайн (с эмбеддингами bge-m3 :8083) ===");
    eprintln!("файлов: {n}  (генерация {gen_ms} мс), чанков: {chunks}");
    eprintln!("ИНДЕКСАЦИЯ: {idx_s:.1} с → {files_per_s:.1} файлов/с, {emb_per_s:.0} эмбеддингов/с");
    eprintln!(
        "ПРОЕКЦИЯ на 50k: ~{:.0} с (~{:.1} мин) фоновой индексации",
        proj_50k_s,
        proj_50k_s / 60.0
    );
    eprintln!(
        "ПОИСК (гибрид+эмбеддинг запроса): {search_ms} мс, hits={}",
        hits.len()
    );
    eprintln!(
        "ГРАФ единый (топ-2000): {full_ms} мс — узлов {} рёбер {} truncated {}",
        full.nodes.len(),
        full.edges.len(),
        full.truncated
    );
    eprintln!(
        "ГРАФ локальный (2-hop): {local_ms} мс — узлов {}",
        local.nodes.len()
    );
    eprintln!("==================================================================\n");

    // Санити (числа выше — главный артефакт; жёстких порогов нет, окружение-зависимо).
    assert!(files_per_s > 0.0);
    assert!(!hits.is_empty(), "поиск должен находить на синтетике");
    assert!(!full.nodes.is_empty());
}

/// ЭКСПЕРИМЕНТ (карт-бланш 2026-06-11, BACKLOG «Реранкер»): LLM-реранк топ-выдачи гибрида мелкой
/// моделью (E4B no-think) против baseline-ранжирования. Файлы топ-24 чанков → модель упорядочивает
/// по релевантности вопросу → метрики@8 против обычного ретрива. НЕ гейт: исследование — печатает
/// сравнение, решение о вливании в прод принимается по числам (AC-EVAL-3: ранжирование без eval
/// не менять).
#[tokio::test]
#[ignore = "нужны embedding-сервер и LLM (NEXUS_EMBED_URL/NEXUS_FAST_URL)"]
async fn live_eval_llm_rerank_experiment() {
    use crate::ai::{default_prefixes, ChatMessage, ChatProvider, OpenAiChatProvider};
    use crate::net::{EgressFeature, GuardedClient};
    use std::collections::HashSet;
    use std::sync::atomic::AtomicBool;

    let golden = load_golden();
    let baseline = load_baseline();
    let cond = &baseline.conditions;
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();

    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(OpenAiEmbedder::new(
        &GuardedClient::unchecked(),
        EgressFeature::Embed,
        &live_embed_url(cond),
        &cond.embedding_model,
        cond.embedding_dim,
        default_prefixes(&cond.embedding_model),
    ));
    let vectors = Arc::new(
        VectorIndex::open(root.join(".nexus/vectors.usearch"), cond.embedding_dim).unwrap(),
    );
    let db = index_corpus(&root, &golden.corpus, embedder.clone(), vectors.clone())
        .await
        .unwrap();

    let fast_url =
        std::env::var("NEXUS_FAST_URL").unwrap_or_else(|_| "http://192.168.0.31:8084".into());
    let reranker = OpenAiChatProvider::new(
        &GuardedClient::unchecked(),
        EgressFeature::Chat,
        &fast_url,
        "rerank-e4b",
        Some(0.0),
    )
    .without_reasoning();
    let cancel = Arc::new(AtomicBool::new(false));

    const RETRIEVE: usize = 24; // глубина кандидатов для реранка
    let k = cond.k; // метрики на тех же k=8, что baseline

    let (mut b_r, mut b_n, mut b_m) = (0f32, 0f32, 0f32);
    let (mut r_r, mut r_n, mut r_m) = (0f32, 0f32, 0f32);
    let n_cases = golden.cases.len() as f32;

    for case in &golden.cases {
        let hits = search::hybrid_search(
            db.reader(),
            Some(&vectors),
            Some(embedder.as_ref()),
            case.query.clone(),
            SearchOptions {
                limit: RETRIEVE,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // Чанки → уникальные файлы (первый сниппет файла — представитель для модели).
        let mut seen = HashSet::new();
        let files: Vec<(String, String)> = hits
            .into_iter()
            .filter_map(|h| seen.insert(h.path.clone()).then_some((h.path, h.snippet)))
            .collect();
        let base_ranked: Vec<String> = files.iter().map(|(p, _)| p.clone()).collect();
        let relevant: HashSet<String> = case.relevant.iter().cloned().collect();
        b_r += recall_at_k(&base_ranked, &relevant, k);
        b_n += ndcg_at_k(&base_ranked, &relevant, k);
        b_m += reciprocal_rank(&base_ranked, &relevant);

        // LLM-реранк: нумерованные кандидаты → JSON-массив номеров по релевантности.
        let mut listing = String::new();
        for (i, (path, snip)) in files.iter().enumerate() {
            let cut: String = snip.chars().take(240).collect();
            listing.push_str(&format!("[{}] {path}: {cut}\n", i + 1));
        }
        let messages = [
            ChatMessage::system(
                "Ты ранжируешь фрагменты заметок по релевантности вопросу. Ответь СТРОГО \
                 JSON-массивом номеров фрагментов от самого релевантного к наименее, без \
                 пояснений: [3,1,2,...]. Включи каждый номер ровно один раз.",
            ),
            ChatMessage::user(format!("Вопрос: {}\n\nФрагменты:\n{listing}", case.query)),
        ];
        let mut out = String::new();
        reranker
            .stream_chat(&messages, &mut |t| out.push_str(&t), &cancel)
            .await
            .expect("реранк-вызов");
        // Парс: первый JSON-массив чисел; недостающие индексы — хвостом в исходном порядке.
        let order: Vec<usize> = out
            .find('[')
            .and_then(|a| out[a..].find(']').map(|b| &out[a + 1..a + b]))
            .map(|inner| {
                inner
                    .split(',')
                    .filter_map(|x| x.trim().parse::<usize>().ok())
                    .filter(|i| *i >= 1 && *i <= files.len())
                    .map(|i| i - 1)
                    .collect()
            })
            .unwrap_or_default();
        let mut used = HashSet::new();
        let mut reranked: Vec<String> = order
            .into_iter()
            .filter(|i| used.insert(*i))
            .map(|i| files[i].0.clone())
            .collect();
        for (i, (p, _)) in files.iter().enumerate() {
            if !used.contains(&i) {
                reranked.push(p.clone());
            }
        }
        r_r += recall_at_k(&reranked, &relevant, k);
        r_n += ndcg_at_k(&reranked, &relevant, k);
        r_m += reciprocal_rank(&reranked, &relevant);
    }

    eprintln!(
        "\n=== LLM-RERANK EXPERIMENT (retrieve={RETRIEVE}, метрики@{k}, n={}) ===\n\
         base   : recall={:.3} nDCG={:.3} MRR={:.3}\n\
         rerank : recall={:.3} nDCG={:.3} MRR={:.3}",
        golden.cases.len(),
        b_r / n_cases,
        b_n / n_cases,
        b_m / n_cases,
        r_r / n_cases,
        r_n / n_cases,
        r_m / n_cases,
    );
}
