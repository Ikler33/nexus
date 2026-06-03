//! Eval-харнесс качества RAG (§6.6, **AC-EVAL-1..6**). По образцу `sa-eval`: golden-набор
//! `вопрос → ожидаемые файлы`, метрики **recall@k / nDCG@k / MRR**, сравнение с зафиксированным
//! baseline (регресс-гейт AC-EVAL-3). Условия прогона (модель/сервер/набор) — в отчёте (AC-EVAL-4).
//!
//! Метрики бинарной релевантности на уровне ФАЙЛОВ (выдача чанков схлопывается в файлы). Прогон —
//! `run_eval` над уже проиндексированным vault; сборка корпуса в temp-vault — `index_corpus`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ai::EmbeddingProvider;
use crate::db::{Database, DbResult};
use crate::indexer::Indexer;
use crate::search::{self, SearchOptions};
use crate::vector::VectorIndex;

// ─── Метрики (бинарная релевантность; ranked — пути в порядке выдачи) ─────────────────────────────

/// Доля релевантных, попавших в топ-`k` (recall@k).
pub fn recall_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f32 {
    if relevant.is_empty() {
        return 0.0;
    }
    let found = ranked
        .iter()
        .take(k)
        .filter(|p| relevant.contains(*p))
        .count();
    found as f32 / relevant.len() as f32
}

/// Reciprocal Rank: 1/(позиция первого релевантного, 1-based); 0, если не найден.
pub fn reciprocal_rank(ranked: &[String], relevant: &HashSet<String>) -> f32 {
    ranked
        .iter()
        .position(|p| relevant.contains(p))
        .map(|i| 1.0 / (i as f32 + 1.0))
        .unwrap_or(0.0)
}

/// nDCG@k для бинарной релевантности (gain 1/0, дисконт 1/log2(rank+1)).
pub fn ndcg_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f32 {
    let dcg: f32 = ranked
        .iter()
        .take(k)
        .enumerate()
        .filter(|(_, p)| relevant.contains(*p))
        .map(|(i, _)| 1.0 / ((i as f32) + 2.0).log2())
        .sum();
    let ideal = relevant.len().min(k);
    let idcg: f32 = (0..ideal).map(|i| 1.0 / ((i as f32) + 2.0).log2()).sum();
    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

// ─── Golden-набор / baseline (данные) ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct GoldenDoc {
    pub path: String,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoldenCase {
    pub query: String,
    pub relevant: Vec<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoldenSet {
    pub corpus: Vec<GoldenDoc>,
    pub cases: Vec<GoldenCase>,
}

/// Зашитый golden-набор (`eval/golden.json`).
pub fn load_golden() -> GoldenSet {
    serde_json::from_str(include_str!("../../eval/golden.json")).expect("eval/golden.json валиден")
}

#[derive(Debug, Clone, Deserialize)]
pub struct BaselineMetrics {
    pub recall_at_k: f32,
    pub ndcg_at_k: f32,
    pub mrr: f32,
}

/// Условия прогона (AC-EVAL-4): сравнение метрик валидно только при их совпадении.
#[derive(Debug, Clone, Deserialize)]
pub struct Conditions {
    pub embedding_model: String,
    pub embedding_server: String,
    pub embedding_dim: usize,
    pub k: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Baseline {
    pub conditions: Conditions,
    pub metrics: BaselineMetrics,
}

/// Зашитый baseline (`eval/baseline.json`) — пороги регресс-гейта (AC-EVAL-2/3).
pub fn load_baseline() -> Baseline {
    serde_json::from_str(include_str!("../../eval/baseline.json"))
        .expect("eval/baseline.json валиден")
}

// ─── Прогон ──────────────────────────────────────────────────────────────────────────────────────

/// Результат одного кейса (для отчёта/диагностики).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseResult {
    pub query: String,
    pub note: String,
    pub recall_at_k: f32,
    pub ndcg_at_k: f32,
    pub reciprocal_rank: f32,
    /// Топ-`k` путей выдачи (для разбора промахов).
    pub hits: Vec<String>,
}

/// Агрегированный отчёт прогона. Условия (модель/сервер) проставляет вызывающий (AC-EVAL-4).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalReport {
    pub k: usize,
    pub n_cases: usize,
    pub recall_at_k: f32,
    pub ndcg_at_k: f32,
    pub mrr: f32,
    pub cases: Vec<CaseResult>,
}

/// Прогоняет golden-кейсы через гибридный поиск и считает агрегированные метрики (recall@k/nDCG/MRR).
pub async fn run_eval(
    reader: &crate::db::ReadPool,
    vectors: &VectorIndex,
    embedder: &dyn EmbeddingProvider,
    cases: &[GoldenCase],
    k: usize,
) -> DbResult<EvalReport> {
    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        let hits = search::hybrid_search(
            reader,
            Some(vectors),
            Some(embedder),
            case.query.clone(),
            SearchOptions {
                limit: k,
                ..Default::default()
            },
        )
        .await?;
        // Чанки → уникальные файлы в порядке выдачи.
        let mut seen = HashSet::new();
        let ranked: Vec<String> = hits
            .into_iter()
            .map(|h| h.path)
            .filter(|p| seen.insert(p.clone()))
            .collect();
        let relevant: HashSet<String> = case.relevant.iter().cloned().collect();
        results.push(CaseResult {
            query: case.query.clone(),
            note: case.note.clone(),
            recall_at_k: recall_at_k(&ranked, &relevant, k),
            ndcg_at_k: ndcg_at_k(&ranked, &relevant, k),
            reciprocal_rank: reciprocal_rank(&ranked, &relevant),
            hits: ranked.into_iter().take(k).collect(),
        });
    }

    let n = results.len().max(1) as f32;
    let mean = |f: &dyn Fn(&CaseResult) -> f32| results.iter().map(f).sum::<f32>() / n;
    Ok(EvalReport {
        k,
        n_cases: results.len(),
        recall_at_k: mean(&|r| r.recall_at_k),
        ndcg_at_k: mean(&|r| r.ndcg_at_k),
        mrr: mean(&|r| r.reciprocal_rank),
        cases: results,
    })
}

/// Строит temp-vault из корпуса golden-набора и индексирует его (RAG). Возвращает БД (reader внутри).
pub async fn index_corpus(
    root: &Path,
    docs: &[GoldenDoc],
    embedder: Arc<dyn EmbeddingProvider>,
    vectors: Arc<VectorIndex>,
) -> DbResult<Database> {
    for doc in docs {
        if let Some(parent) = Path::new(&doc.path).parent() {
            std::fs::create_dir_all(root.join(parent)).ok();
        }
        std::fs::write(root.join(&doc.path), &doc.body)?;
    }
    let db = Database::open(root.join(".nexus/nexus.db")).await?;
    let idx = Indexer::with_rag(&db, root.to_path_buf(), embedder, vectors, true);
    for doc in docs {
        idx.index_file(&doc.path).await?;
    }
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{EmbeddingProvider, MockEmbedder, OpenAiEmbedder};
    use tempfile::TempDir;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn recall_counts_relevant_in_top_k() {
        let ranked: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(recall_at_k(&ranked, &set(&["b", "d"]), 8), 1.0);
        assert_eq!(recall_at_k(&ranked, &set(&["b", "z"]), 8), 0.5);
        assert_eq!(recall_at_k(&ranked, &set(&["c"]), 2), 0.0); // c на позиции 3, k=2
        assert_eq!(recall_at_k(&ranked, &HashSet::new(), 8), 0.0);
    }

    #[test]
    fn rr_is_inverse_of_first_relevant_rank() {
        let ranked: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(reciprocal_rank(&ranked, &set(&["a"])), 1.0);
        assert_eq!(reciprocal_rank(&ranked, &set(&["c"])), 1.0 / 3.0);
        assert_eq!(reciprocal_rank(&ranked, &set(&["z"])), 0.0);
    }

    #[test]
    fn ndcg_rewards_higher_rank() {
        let top: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let low: Vec<String> = ["x", "y", "a"].iter().map(|s| s.to_string()).collect();
        let rel = set(&["a"]);
        assert!((ndcg_at_k(&top, &rel, 8) - 1.0).abs() < 1e-6); // релевантный первый → 1.0
        assert!(ndcg_at_k(&low, &rel, 8) < 1.0); // ниже → меньше
        assert!(ndcg_at_k(&low, &rel, 8) > 0.0);
    }

    #[test]
    fn golden_and_baseline_parse() {
        let g = load_golden();
        assert!(g.corpus.len() >= 10 && !g.cases.is_empty());
        let b = load_baseline();
        assert!(b.metrics.recall_at_k > 0.0 && b.metrics.recall_at_k <= 1.0);
    }

    /// Детерминированный сквозной прогон харнесса на mock: FTS по точному слову гарантирует, что
    /// нужный файл попадает в top-k → recall=1, MRR=1 (проверяет проводку run_eval, не семантику).
    #[tokio::test]
    async fn run_eval_wires_end_to_end_with_mock() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let docs = vec![
            GoldenDoc {
                path: "alpha.md".into(),
                body: "# A\n\nzzqunique alpha content".into(),
            },
            GoldenDoc {
                path: "beta.md".into(),
                body: "# B\n\nordinary beta words".into(),
            },
            GoldenDoc {
                path: "gamma.md".into(),
                body: "# G\n\nother gamma text".into(),
            },
        ];
        let vectors = Arc::new(VectorIndex::open(root.join(".nexus/vectors.usearch"), 16).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
        let db = index_corpus(&root, &docs, embedder.clone(), vectors.clone())
            .await
            .unwrap();

        let cases = vec![GoldenCase {
            query: "zzqunique".into(),
            relevant: vec!["alpha.md".into()],
            note: "exact-term".into(),
        }];
        let report = run_eval(db.reader(), &vectors, embedder.as_ref(), &cases, 8)
            .await
            .unwrap();
        assert_eq!(report.n_cases, 1);
        assert_eq!(report.recall_at_k, 1.0, "точное слово → файл в top-k");
        assert_eq!(report.mrr, 1.0);
        assert!(report.cases[0].hits.contains(&"alpha.md".to_string()));
    }

    /// Живой прогон на nomic :8081 (`cargo test -- --ignored`): печатает отчёт и проверяет, что
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
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            OpenAiEmbedder::new(
                &cond.embedding_server,
                &cond.embedding_model,
                cond.embedding_dim,
                default_prefixes(&cond.embedding_model),
            )
            .unwrap(),
        );
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
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            OpenAiEmbedder::new(
                "http://127.0.0.1:8083",
                "bge-m3",
                1024,
                default_prefixes("bge-m3"),
            )
            .unwrap(),
        );
        let vectors =
            Arc::new(VectorIndex::open(tmp.path().join("vectors.usearch"), 1024).unwrap());
        let db = Database::open(tmp.path().join("nexus.db")).await.unwrap();
        let indexer = Indexer::with_rag(&db, root.clone(), embedder.clone(), vectors.clone(), true);
        indexer.scan_vault().await.unwrap();

        // (запрос, ожидаемый файл-подстрока). Первые две — кросс-язычные (RU-запрос → EN-заметка).
        let probes = [
            ("рецепт хлеба на закваске", "Sourdough"),
            (
                "борьба с утечками памяти без сборщика мусора",
                "Rust-Ownership",
            ),
            (
                "how does approximate nearest neighbour search work",
                "Vector-Search",
            ),
            (
                "права плагинов, аудит и предотвращение confused deputy",
                "Безопасность",
            ),
        ];
        let mut ok = 0;
        for (q, expect) in probes {
            let hits = search::hybrid_search(
                db.reader(),
                Some(&vectors),
                Some(embedder.as_ref()),
                q.to_string(),
                SearchOptions {
                    limit: 8,
                    filter: None,
                    center: None,
                },
            )
            .await
            .unwrap();
            let rank = hits.iter().position(|h| h.path.contains(expect));
            ok += usize::from(rank.is_some());
            let top: Vec<String> = hits
                .iter()
                .map(|h| format!("{}({:.3})", h.path, h.score))
                .collect();
            eprintln!(
                "[{}] {q:?} → rank={rank:?}\n      {top:?}",
                if rank.is_some() { "OK" } else { "--" }
            );
        }
        assert!(ok >= 3, "ожидали ≥3/4 проб найденными, получили {ok}/4");
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
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            OpenAiEmbedder::new(
                "http://127.0.0.1:8083",
                "bge-m3",
                1024,
                default_prefixes("bge-m3"),
            )
            .unwrap(),
        );
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus/vectors.usearch"), 1024).unwrap());
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
        eprintln!(
            "ИНДЕКСАЦИЯ: {idx_s:.1} с → {files_per_s:.1} файлов/с, {emb_per_s:.0} эмбеддингов/с"
        );
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
}
