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

/// **Cold-bench ЛОКАЛЬНОГО пайплайна (#19)** — масштабирование БЕЗ сети: эмбеддинги мокаются
/// (`MockEmbedder`, мгновенные, детерминированные), поэтому изолируем РЕАЛЬНЫЕ узкие места локали —
/// парсинг/чанкинг, запись в FTS5/SQLite (write-actor), построение usearch ANN, латентность
/// гибридного поиска и графа на БОЛЬШИХ N. Сетевой throughput эмбеддинга мерится отдельно
/// (`bench_index_scale` вживую, ~30 чанков/с на риге) — здесь его НЕ ждём, поэтому 50k+ за секунды.
/// Размер: `NEXUS_BENCH_FILES=50000 cargo test bench_local_pipeline_scale -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "cold-bench: тяжёлый (10k–100k файлов); запускать вручную через NEXUS_BENCH_FILES"]
async fn bench_local_pipeline_scale() {
    use crate::ai::MockEmbedder;
    use std::time::Instant;

    let n: usize = std::env::var("NEXUS_BENCH_FILES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);

    // 1) Синтетический vault: N заметок РАЗЛОЖЕНЫ ПО ПАПКАМ (как реальный Obsidian), ссылки —
    //    bare-basename `[[Note-J]]` (шорткат без пути/.md). Это нагружает basename-резолв
    //    (`resolve_target` шаг 2) — самое узкое место на масштабе (#19). 50 папок, ~3 ссылки + теги.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    for d in 0..50 {
        std::fs::create_dir_all(root.join(format!("dir{d}"))).unwrap();
    }
    let gen0 = Instant::now();
    for i in 0..n {
        let body = format!(
            "# Note {i}\n\n\
             Синтетическая заметка номер {i} для cold-bench. Русский текст и some English text про \
             vector search, knowledge base и retrieval augmented generation. Второй параграф: \
             индексация, эмбеддинги, гибридный поиск, граф связей, чанкинг, FTS5, usearch.\n\n\
             Связи: [[Note-{}]] [[Note-{}]] [[Note-{}]]\n\n#bench #note{}\n",
            (i + 1) % n,
            (i + 7) % n,
            (i + 53) % n,
            i % 20,
        );
        std::fs::write(root.join(format!("dir{}/Note-{i}.md", i % 50)), body).unwrap();
    }
    let gen_s = gen0.elapsed().as_secs_f64();

    // 2) Пайплайн с МОК-эмбеддером (эмбеддинг мгновенный → меряем только локаль).
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 1024 });
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

    // Размер артефактов на диске (масштабируется ли память/диск линейно).
    let db_mb = std::fs::metadata(root.join(".nexus/nexus.db"))
        .map(|m| m.len() as f64 / 1.048576e6)
        .unwrap_or(0.0);
    let usearch_mb = std::fs::metadata(root.join(".nexus/vectors.usearch"))
        .map(|m| m.len() as f64 / 1.048576e6)
        .unwrap_or(0.0);

    // 3) Латентность поиска: K запросов с РАЗНЫМ текстом (без кэш-артефактов) → p50/p95/max.
    let mut lat = Vec::new();
    for k in 0..20 {
        let q = format!("vector search граф связей чанкинг note {k}");
        let t = Instant::now();
        let hits = search::hybrid_search(
            db.reader(),
            Some(&vectors),
            Some(embedder.as_ref()),
            q,
            SearchOptions {
                limit: 8,
                filter: None,
                center: None,
            },
        )
        .await
        .unwrap();
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(!hits.is_empty());
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = lat[lat.len() / 2];
    let p95 = lat[(lat.len() * 95 / 100).min(lat.len() - 1)];
    let pmax = *lat.last().unwrap();

    // 4) Граф: единый топ-2000 + локальный 2-hop.
    let fg0 = Instant::now();
    let full = crate::graph::get_full_graph(db.reader(), 2000)
        .await
        .unwrap();
    let full_ms = fg0.elapsed().as_millis();
    let lg0 = Instant::now();
    let local = crate::graph::get_local_graph(db.reader(), "dir0/Note-0.md".to_string(), 2)
        .await
        .unwrap();
    let local_ms = lg0.elapsed().as_millis();

    eprintln!("\n=== NEXUS cold-bench: ЛОКАЛЬНЫЙ пайплайн (мок-эмбеддинг, без сети) ===");
    eprintln!("файлов: {n} (генерация {gen_s:.1} с), чанков: {chunks}");
    eprintln!(
        "ИНДЕКСАЦИЯ (parse+chunk+FTS5+usearch): {idx_s:.1} с → {:.0} файлов/с, {:.0} чанков/с",
        n as f64 / idx_s,
        chunks as f64 / idx_s
    );
    eprintln!("ДИСК: nexus.db {db_mb:.1} МБ, vectors.usearch {usearch_mb:.1} МБ");
    eprintln!("ПОИСК (гибрид, 20 запросов): p50 {p50:.0} мс, p95 {p95:.0} мс, max {pmax:.0} мс");
    eprintln!(
        "ГРАФ: единый(топ-2000) {full_ms} мс [узлов {} рёбер {} trunc {}], локальный(2-hop) {local_ms} мс [узлов {}]",
        full.nodes.len(),
        full.edges.len(),
        full.truncated,
        local.nodes.len()
    );
    eprintln!("================================================================\n");

    assert!(chunks as usize >= n, "каждая заметка дала ≥1 чанк");
    assert!(
        p95 < 5000.0,
        "поиск p95 {p95:.0} мс — деградация на масштабе N={n}"
    );
}

/// ЭКСПЕРИМЕНТ (карт-бланш 2026-06-11, BACKLOG «Реранкер»): LLM-реранк топ-выдачи гибрида мелкой
/// моделью (E4B no-think) против baseline-ранжирования. Файлы топ-24 чанков → модель упорядочивает
/// по релевантности вопросу → метрики@8 против обычного ретрива. НЕ гейт: исследование — печатает
/// сравнение, решение о вливании в прод принимается по числам (AC-EVAL-3: ранжирование без eval
/// не менять).
#[tokio::test]
#[ignore = "нужны embedding-сервер и LLM (NEXUS_EMBED_URL/NEXUS_FAST_URL)"]
async fn live_eval_llm_rerank_experiment() {
    use crate::ai::{default_prefixes, ChatProvider, OpenAiChatProvider};
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

        // LLM-реранк: нумерованные кандидаты → JSON-массив номеров по релевантности. Промпт строит
        // ЕДИНАЯ прод-функция `rerank::build_rerank_messages` (тот же per-request маркер и ограждение,
        // AC-SEC-7) — eval мерит РЕАЛЬНЫЙ прод-промпт, не ручную копию (защита от дрейфа, P0-e).
        let fragments: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, snip)| (path.as_str(), snip.as_str()))
            .collect();
        let messages = crate::search::rerank::build_rerank_messages(&case.query, &fragments);
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

/// AI-2c (A4) live-гейт: прогоняет РЕАЛЬНЫЙ `chat_util`-классификатор (Qwen3-4B :8084) по зашитой
/// `eval/tag_golden.json` и проверяет closed-vocab-харнесс — `out_of_vocab==0` И микро-precision/recall не
/// ниже порогов (§10 A4). Это «боевая» точка подключения, дополняющая детерминированный
/// `fixture_runs_through_gate_and_discriminates` (тот в CI проверяет, что гейт ловит регресс БЕЗ LLM).
/// Запуск: `NEXUS_FAST_URL=http://192.168.0.31:8084 cargo test live_classify_tags_meets_gate -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "live-llm авто-тег (AI-2c): нужен chat_util (NEXUS_FAST_URL / 192.168.0.31:8084)"]
async fn live_classify_tags_meets_gate() {
    use super::classify::{
        evaluate_tags, load_tag_golden, meets_thresholds, MIN_PRECISION, MIN_RECALL,
    };
    use crate::ai::{ChatProvider, OpenAiChatProvider};
    use crate::net::{EgressFeature, GuardedClient};
    use std::collections::HashSet;
    use std::sync::atomic::AtomicBool;

    let fast_url =
        std::env::var("NEXUS_FAST_URL").unwrap_or_else(|_| "http://192.168.0.31:8084".into());
    let chat: Arc<dyn ChatProvider> = Arc::new(
        OpenAiChatProvider::new(
            &GuardedClient::unchecked(),
            EgressFeature::Chat,
            &fast_url,
            "qwen3-4b",
            Some(0.0),
        )
        .without_reasoning(),
    );
    let cancel = Arc::new(AtomicBool::new(false));

    let golden = load_tag_golden();
    let vocab_set: HashSet<String> = golden.vocabulary.iter().cloned().collect();

    let mut predictions: Vec<(String, HashSet<String>)> = Vec::new();
    let mut gold: Vec<(String, Vec<String>)> = Vec::new();
    let mut dropped_total = 0usize; // реальные out-of-vocab выдачи модели (до фильтра)
    for case in &golden.cases {
        let s = crate::tagger::classify_tags(&chat, &golden.vocabulary, &case.body, &cancel).await;
        dropped_total += s.dropped;
        predictions.push((case.path.clone(), s.tags.into_iter().collect()));
        gold.push((case.path.clone(), case.gold.clone()));
    }

    let report = evaluate_tags(&predictions, &gold, &vocab_set);
    // dropped = сколько тегов модель выдала ВНЕ словаря (отсеяно фильтром); oov по отчёту — всегда 0
    // (production-выход уже vocab-фильтрован), это инвариант-санити, а не «сколько отброшено».
    eprintln!(
        "AI-2c авто-тег: precision={:.3} recall={:.3} f1={:.3} (tp={} fp={} fn={} dropped={} oov={})",
        report.precision,
        report.recall,
        report.f1,
        report.tp,
        report.fp,
        report.fn_count,
        dropped_total,
        report.out_of_vocab,
    );
    assert_eq!(
        report.out_of_vocab, 0,
        "production-выход всегда vocab-отфильтрован"
    );
    assert!(
        meets_thresholds(&report, MIN_PRECISION, MIN_RECALL),
        "авто-тег не прошёл гейт precision≥{MIN_PRECISION}/recall≥{MIN_RECALL}"
    );
}

/// MEM-8c live-гейт консолидации (§4.5): прогоняет РЕАЛЬНУЮ основную модель (`consolidate::decide`,
/// Qwen3 27B :8080) по зашитой `eval/consolidation_eval.json` при t=0 и проверяет DELETE-precision /
/// UPDATE-quality / op-accuracy не ниже порогов. Прохождение РАЗБЛОКИРУЕТ авто-DELETE (MEM-8c) — доверие
/// к авто-удалению фактов = это число на наших данных. Дополняет детерминированные тесты
/// `consolidation.rs` (те в CI ловят регресс гейта БЕЗ LLM).
/// Запуск: `NEXUS_CHAT_URL=http://192.168.0.31:8080 NEXUS_CHAT_MODEL=gemma cargo test live_consolidation_meets_gate -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "live-llm консолидация (MEM-8c): нужна основная модель (NEXUS_CHAT_URL / 192.168.0.31:8080)"]
async fn live_consolidation_meets_gate() {
    use super::consolidation::{
        evaluate_consolidation, load_consolidation_golden, meets_consolidation_gate, OpPrediction,
        MIN_DELETE_PRECISION, MIN_UPDATE_QUALITY,
    };
    use crate::ai::{ChatProvider, OpenAiChatProvider};
    use crate::net::{EgressFeature, GuardedClient};

    let chat_url =
        std::env::var("NEXUS_CHAT_URL").unwrap_or_else(|_| "http://192.168.0.31:8080".into());
    let model = std::env::var("NEXUS_CHAT_MODEL").unwrap_or_else(|_| "gemma".into());
    // t=0 (детерминизм гейта, §4.5) + без reasoning (нужен только JSON-вердикт, не CoT).
    let chat: Arc<dyn ChatProvider> = Arc::new(
        OpenAiChatProvider::new(
            &GuardedClient::unchecked(),
            EgressFeature::Chat,
            &chat_url,
            &model,
            Some(0.0),
        )
        .without_reasoning(),
    );

    let golden = load_consolidation_golden();
    let mut predictions: Vec<OpPrediction> = Vec::with_capacity(golden.cases.len());
    for case in &golden.cases {
        let (op, target, merged) =
            crate::memory::consolidate::decide_eval(&chat, &case.candidate, &case.existing).await;
        predictions.push(OpPrediction {
            op: op.to_string(),
            target,
            merged,
        });
    }

    let report = evaluate_consolidation(&predictions, &golden);
    // op_accuracy / delete_recall — ИНФОРМАЦИОННЫЕ (полезность), НЕ в гейте: консервативная модель
    // (низкий recall, пропуск контрадикций) безопасна — гейт меряет ложные удаления, не промахи.
    eprintln!(
        "MEM-8c консолидация (гейт=DELETE-precision+UPDATE-quality+predicted_delete>0): \
         DELETE-precision={:.3} (correct={} false={} predicted={}/gold={}) UPDATE-quality={:.3} ({}/{}) \
         | инфо: op_accuracy={:.3} delete_recall={:.3}",
        report.delete_precision,
        report.correct_delete,
        report.false_delete,
        report.predicted_delete,
        report.gold_delete,
        report.update_quality,
        report.update_good,
        report.update_cases,
        report.op_accuracy,
        report.delete_recall,
    );
    assert!(
        meets_consolidation_gate(&report, MIN_DELETE_PRECISION, MIN_UPDATE_QUALITY),
        "консолидация не прошла гейт: DELETE-precision≥{MIN_DELETE_PRECISION}, UPDATE-quality≥{MIN_UPDATE_QUALITY}, predicted_delete>0"
    );
}

/// EP-2 БЛОКИРУЮЩИЙ live-гейт faithfulness эпизодов: реальная модель суммирует каждый golden-транскрипт,
/// грейдим (нет галлюцинаций + заземлено), сверяем с порогом. Ретривал эпизодов в чат НЕ включается
/// (EP-2 не мержится), пока этот гейт не зелёный на актуальной модели саммари. При смене модели —
/// рекалибровка ([[project_nexus_consolidation_recalibrate]]).
///
/// Прод-саммари идёт `chat_util`→`chat_fast` (см. open_vault). Запуск против основной (gemma):
/// `NEXUS_CHAT_URL=http://192.168.0.31:8080 NEXUS_CHAT_MODEL=gemma cargo test live_episode_summary_meets_gate -- --ignored --nocapture`
/// против утилитарной: `NEXUS_CHAT_URL=http://192.168.0.31:8084 NEXUS_CHAT_MODEL=<util> ...`.
#[tokio::test]
#[ignore = "live-llm faithfulness эпизодов (EP-2): нужна модель саммари (NEXUS_CHAT_URL / 192.168.0.31)"]
async fn live_episode_summary_meets_gate() {
    use super::episodes::{
        evaluate_episodes, load_episode_golden, meets_episode_gate, transcript_pairs,
        MIN_EPISODE_FAITHFULNESS,
    };
    use crate::ai::{ChatProvider, OpenAiChatProvider};
    use crate::net::{EgressFeature, GuardedClient};

    let chat_url =
        std::env::var("NEXUS_CHAT_URL").unwrap_or_else(|_| "http://192.168.0.31:8080".into());
    let model = std::env::var("NEXUS_CHAT_MODEL").unwrap_or_else(|_| "gemma".into());
    // t≈0.2 (как прод-summarize) + без reasoning (саммари дешёвое, CoT не нужен).
    let chat: Arc<dyn ChatProvider> = Arc::new(
        OpenAiChatProvider::new(
            &GuardedClient::unchecked(),
            EgressFeature::Chat,
            &chat_url,
            &model,
            Some(0.2),
        )
        .without_reasoning(),
    );

    let golden = load_episode_golden();
    let mut summaries: Vec<String> = Vec::with_capacity(golden.cases.len());
    for case in &golden.cases {
        let pairs = transcript_pairs(case);
        // Прод-путь генерации саммари; ошибка/пусто → пустая строка (графится как off-topic, честно).
        let (summary, _topics) = crate::episode::summarize(chat.as_ref(), &pairs)
            .await
            .unwrap_or_default();
        summaries.push(summary);
    }

    let report = evaluate_episodes(&summaries, &golden);
    eprintln!(
        "EP-2 faithfulness (модель={model}): faithfulness={:.3} ({}/{}) | hallucinated={} off_topic={}",
        report.faithfulness, report.faithful, report.cases, report.hallucinated, report.off_topic,
    );
    assert!(
        meets_episode_gate(&report, MIN_EPISODE_FAITHFULNESS),
        "faithfulness ниже {MIN_EPISODE_FAITHFULNESS} — ретривал эпизодов НЕ разблокирован (ложная память)"
    );
}
