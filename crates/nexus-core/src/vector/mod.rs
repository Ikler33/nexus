//! Векторный ANN-индекс (usearch HNSW) — sibling-файл `.nexus/vectors.usearch`.
//!
//! Ключ = `chunk_id` (u64) из таблицы `chunks`; размерность = `embedder.dim()` (НЕ хардкод —
//! §5/§6.5). Cosine-метрика (векторы L2-нормализованы эмбеддером). `upsert` заменяет вектор
//! по ключу (нет дублей при реиндексации — AC-Б4-2); `remove` чистит при удалении (AC-Б8-2).
//! Транзакционность с SQLite — на уровне write-actor (usearch — отдельный файл), reconcile §5.1.

use std::path::{Path, PathBuf};

use thiserror::Error;
use usearch::ffi::Matches;
use usearch::{new_index, Index, IndexOptions, MetricKind, ScalarKind};

#[derive(Debug, Error)]
pub enum VectorError {
    #[error("usearch: {0}")]
    Usearch(String),
    #[error("размерность вектора: ожидалось {expected}, получено {got}")]
    DimMismatch { expected: usize, got: usize },
    #[error("путь не UTF-8: {0}")]
    BadPath(String),
}

pub type VectorResult<T> = Result<T, VectorError>;

/// Результат ANN-поиска: `chunk_id` + similarity (1 − cosine-distance, выше = ближе).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorHit {
    pub chunk_id: u64,
    pub score: f32,
}

/// Общий каркас векторного ретривала для ОДНОРОДНЫХ вызывателей (факты MEM `context_facts`,
/// эпизоды EP-2 `search_episodes`, память переписки N4 `search_memory`, консолидация MEM
/// `plan_consolidation`): эмбеддит текстовый запрос и берёт top-`overfetch` из индекса, унифицируя
/// повторяющуюся связку `embed_query → map_err(External) → search → map_err(External)`.
///
/// `overfetch` и порог отсева — ПАРАМЕТРЫ вызывающего, НЕ унифицированы: overfetch у трёх поисков
/// `(k*4).max(8)` (запас на пост-фильтр порогом/дедупом/исключением сессии), у консолидации —
/// фиксированный `CONSOLIDATE_S`; порог свой у каждого (MEM/EPISODE/CHAT_MEM/CONSOLIDATE). Отсев по
/// порогу — отдельными [`ids_above_threshold`](crate::memory::ids_above_threshold) (только id) /
/// [`hits_above_threshold`] (id+score). web/pinned — НЕ сюда (негомогенны: web=abort+замещение,
/// pinned=не поиск).
pub(crate) async fn embed_and_search(
    vectors: &VectorIndex,
    embedder: &dyn crate::ai::EmbeddingProvider,
    query: &str,
    overfetch: usize,
) -> crate::db::DbResult<Vec<VectorHit>> {
    let qvec = embedder
        .embed_query(query)
        .await
        .map_err(|e| crate::db::DbError::External(e.to_string()))?;
    vectors
        .search(&qvec, overfetch)
        .map_err(|e| crate::db::DbError::External(e.to_string()))
}

/// Пары `(chunk_id, score)` хитов с `score ≥ threshold`, в порядке ранга (хиты уже отсортированы по
/// убыванию score). Вариант с сохранением score для вызывателей, резолвящих с рангом (эпизоды/память
/// переписки); id-only вариант — [`ids_above_threshold`](crate::memory::ids_above_threshold).
pub(crate) fn hits_above_threshold(hits: Vec<VectorHit>, threshold: f32) -> Vec<(i64, f32)> {
    hits.into_iter()
        .filter(|h| h.score >= threshold)
        .map(|h| (h.chunk_id as i64, h.score))
        .collect()
}

/// ANN-индекс поверх usearch.
pub struct VectorIndex {
    index: Index,
    dim: usize,
    path: PathBuf,
}

fn usearch_err<E: std::fmt::Display>(e: E) -> VectorError {
    VectorError::Usearch(e.to_string())
}

/// usearch `Matches` → `Vec<VectorHit>` (similarity = 1 − cos-distance).
fn hits_of(matches: Matches) -> Vec<VectorHit> {
    matches
        .keys
        .into_iter()
        .zip(matches.distances)
        .map(|(chunk_id, dist)| VectorHit {
            chunk_id,
            score: 1.0 - dist,
        })
        .collect()
}

impl VectorIndex {
    /// Открывает индекс по пути (загружает существующий) или создаёт новый под `dim`.
    pub fn open(path: impl AsRef<Path>, dim: usize) -> VectorResult<Self> {
        let options = IndexOptions {
            dimensions: dim,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            connectivity: 0,     // 0 → дефолт usearch
            expansion_add: 0,    // 0 → дефолт
            expansion_search: 0, // 0 → дефолт
            multi: false,
        };
        let index = new_index(&options).map_err(usearch_err)?;
        let path = path.as_ref().to_path_buf();
        let path_str = path
            .to_str()
            .ok_or_else(|| VectorError::BadPath(path.display().to_string()))?;

        if path.exists() {
            index.load(path_str).map_err(usearch_err)?;
        } else {
            index.reserve(1024).map_err(usearch_err)?;
        }
        Ok(Self { index, dim, path })
    }

    fn ensure_capacity(&self) -> VectorResult<()> {
        if self.index.size() + 1 > self.index.capacity() {
            let next = (self.index.capacity().max(1024)) * 2;
            self.index.reserve(next).map_err(usearch_err)?;
        }
        Ok(())
    }

    /// Вставляет/заменяет вектор по ключу (chunk_id). Замена снимает старый → нет дублей (AC-Б4-2).
    pub fn upsert(&self, chunk_id: u64, vector: &[f32]) -> VectorResult<()> {
        if vector.len() != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        if self.index.contains(chunk_id) {
            self.index.remove(chunk_id).map_err(usearch_err)?;
        }
        self.ensure_capacity()?;
        self.index.add(chunk_id, vector).map_err(usearch_err)?;
        Ok(())
    }

    /// Удаляет вектор по ключу (no-op, если отсутствует) — чистка при удалении файла (AC-Б8-2).
    pub fn remove(&self, chunk_id: u64) -> VectorResult<()> {
        self.index.remove(chunk_id).map_err(usearch_err)?;
        Ok(())
    }

    /// ANN-поиск top-`k` ближайших (similarity = 1 − cos-distance).
    pub fn search(&self, query: &[f32], k: usize) -> VectorResult<Vec<VectorHit>> {
        if query.len() != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: query.len(),
            });
        }
        let matches = self.index.search(query, k).map_err(usearch_err)?;
        Ok(hits_of(matches))
    }

    /// ANN-поиск top-`k` с предикатом `allow(chunk_id)`: фильтр применяется ВНУТРИ обхода HNSW
    /// (настоящий префильтр ДО отбора результатов — usearch `filtered_search`), а не пост-фильтром
    /// (тот терял бы recall при селективном фильтре). База для метаданного префильтра (AC-Б6-2).
    pub fn search_filtered(
        &self,
        query: &[f32],
        k: usize,
        allow: impl Fn(u64) -> bool,
    ) -> VectorResult<Vec<VectorHit>> {
        if query.len() != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: query.len(),
            });
        }
        let matches = self
            .index
            .filtered_search(query, k, allow)
            .map_err(usearch_err)?;
        Ok(hits_of(matches))
    }

    /// Возвращает сохранённый вектор по `chunk_id` (для suggest: переиспользуем уже посчитанные
    /// эмбеддинги, без обращения к серверу). `None`, если ключа нет в индексе.
    pub fn get_vector(&self, chunk_id: u64) -> VectorResult<Option<Vec<f32>>> {
        if !self.index.contains(chunk_id) {
            return Ok(None);
        }
        let mut buf = vec![0f32; self.dim];
        let n = self.index.get(chunk_id, &mut buf).map_err(usearch_err)?;
        Ok((n >= 1).then_some(buf))
    }

    /// Сохраняет индекс на диск (sibling-файл).
    pub fn save(&self) -> VectorResult<()> {
        let path_str = self
            .path
            .to_str()
            .ok_or_else(|| VectorError::BadPath(self.path.display().to_string()))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(usearch_err)?;
        }
        self.index.save(path_str).map_err(usearch_err)
    }

    pub fn len(&self) -> usize {
        self.index.size()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, chunk_id: u64) -> bool {
        self.index.contains(chunk_id)
    }

    pub fn dim(&self) -> usize {
        self.dim
    }
}

/// Имена sibling-файлов ВСЕХ векторных индексов vault (в `.nexus/`). Один источник, чтобы reconcile
/// и открыватели не разъехались по строкам. Порядок: note-RAG, переписка (N4b), факты (MEM), эпизоды (EP).
pub const VECTOR_INDEX_FILES: &[&str] = &[
    "vectors.usearch",
    "chat_vectors.usearch",
    "memory_vectors.usearch",
    "episode_vectors.usearch",
];

/// КАНОН reconcile embedding-модели (R-3d, решение владельца §8.5): гард совместимости ВСЕХ
/// производных vault (chunks + векторные индексы) с активной моделью/размерностью. Единственная
/// реализация — desktop `open_vault`/`build_rag` и agentd `build_rag_min` зовут её ДО открытия
/// usearch-индексов. До R-3d жили две реплики-«подмножества» (desktop чистил chunks + 3 индекса
/// БЕЗ chat_vectors; эта — 4 индекса БЕЗ chunks); канон = superset обеих (см. CHANGELOG R-3d).
///
/// Семантика (§6.5 / CORE-2a #2):
/// - **та же модель/dim** → СТРОГИЙ no-op, `Ok(false)`: кроме чтения `settings` ничего не пишется
///   и не удаляется — пользовательские индексы НЕ пересобираются на ровном месте;
/// - **первое включение** (settings пусты) → только запись `settings`, `Ok(true)` (индексация
///   с нуля; производных ещё нет — существующие файлы НЕ трогаем, сноса нет);
/// - **смена модели/dim** → ПОЛНАЯ чистка производных: `DELETE FROM chunks` (+FTS триггерами;
///   перезаполнит переиндексация), снос ВСЕХ файлов [`VECTOR_INDEX_FILES`] (перезаполнят
///   индексатор/бэкфиллы новой моделью), `chat_episodes.embed_model = NULL` (эпизоды на
///   переэмбеддинг, summary-текст цел), `files.size_bytes = -1` (durable-маркер, ниже),
///   запись новых `settings`, `Ok(true)`.
///
/// Иначе запрос НОВОЙ моделью против СТАРЫХ векторов/чанков даёт `DimMismatch` или семантический
/// мусор (ложная память) — класс «старые хвосты в поиске/памяти после смены эмбеддера» закрыт.
///
/// Возвращает `Ok(reindex_needed)` — маркер принудительной (пере)индексации: desktop передаёт его
/// в `Indexer::with_rag(force)` (начальный скан игнорирует mtime-шорткат), agentd логирует.
/// **Durable-маркер** (adversarial-ревью R-3d): bool-возврат живёт только в процессе вызвавшего,
/// а реконсилить может процесс БЕЗ индексатора заметок (agentd) — desktop потом откроет vault уже
/// под новой моделью (no-op, `force=false`), и mtime+size-шорткат скана пропустил бы нетронутые
/// файлы НАВСЕГДА (chunks/FTS пусты). Поэтому чистка дополнительно ломает шорткат
/// (`files.size_bytes = -1`; настоящий размер неотрицателен): ЛЮБОЙ следующий скан перечанкует
/// всё независимо от force — переиндексация доедет, какой бы вызыватель ни реконсилил и в каком
/// бы месте ни случился крах. Ошибки БД — `Err` (вызывающий деградирует: RAG/память off).
pub async fn reconcile_embedding_model(
    db: &crate::db::Database,
    root: &Path,
    model: &str,
    dim: usize,
) -> crate::db::DbResult<bool> {
    use rusqlite::OptionalExtension;

    let read_setting = |key: &'static str| {
        let reader = db.reader().clone();
        async move {
            reader
                .query(move |c| {
                    c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()
                })
                .await
        }
    };
    let prev_model = read_setting("embedding.model").await?;
    let prev_dim = read_setting("embedding.dim").await?;

    if prev_model.as_deref() == Some(model) && prev_dim.as_deref() == Some(&dim.to_string()) {
        return Ok(false); // та же модель/dim — инкрементально, индексы совместимы
    }

    if prev_model.is_some() {
        // Модель/dim сменились → ВСЕ производные несовместимы (R-3d, §8.5: «полная чистка»).
        // Крах-безопасность: чистка идёт ДО записи settings — крах до неё оставит prev-настройки
        // (следующий запуск повторит чистку, идемпотентно); крах ПОСЛЕ записи settings безопасен
        // из-за durable-маркера size_bytes=-1 (файлы остаются помеченными → скан доиндексирует).
        for f in VECTOR_INDEX_FILES {
            let path = root.join(".nexus").join(f);
            // NotFound — норма (индекс мог не существовать). Прочий Err (напр. открытый Windows-
            // хендл) значим: выживший usearch несёт ВЕКТОРЫ СТАРОЙ МОДЕЛИ — при другом dim это
            // перманентный RAG-off. Не глушим молча (MINOR-1 ревью): warn для диагностики; durable
            // size_bytes=-1 ниже всё равно форсирует переиндексацию → orphan-ключи вытеснятся.
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(
                    "reconcile: не удалить старый usearch {}: {e} — переиндексация форсируется маркером, но проверь файл",
                    path.display()
                ),
            }
        }
        db.writer()
            .transaction(|tx| {
                // chunks эмбеддились старой моделью (FTS-производную чистят триггеры);
                // эпизоды — на переэмбеддинг бэкфиллом (summary-текст цел).
                tx.execute("DELETE FROM chunks", [])?;
                tx.execute("UPDATE chat_episodes SET embed_model=NULL", [])?;
                // Durable-маркер переиндексации (см. док выше): ломаем mtime+size-шорткат скана,
                // чтобы chunks гарантированно пересоздались, даже если true-маркер потребил
                // процесс без индексатора (agentd) и desktop откроется уже как no-op.
                tx.execute("UPDATE files SET size_bytes=-1", [])?;
                Ok(())
            })
            .await?;
        tracing::info!(from = ?prev_model, to = %model, dim, "смена embedding-модели → полная чистка производных: chunks + все векторные индексы (R-3d, §6.5)");
    }

    let (model_s, dim_s) = (model.to_string(), dim.to_string());
    db.writer()
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO settings(key,value) VALUES('embedding.model',?1) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [model_s],
            )?;
            tx.execute(
                "INSERT INTO settings(key,value) VALUES('embedding.dim',?1) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [dim_s],
            )?;
            Ok(())
        })
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const DIM: usize = 4;

    fn idx(dir: &TempDir) -> VectorIndex {
        VectorIndex::open(dir.path().join(".nexus/vectors.usearch"), DIM).unwrap()
    }

    #[test]
    fn upsert_search_and_no_dup_growth() {
        let dir = TempDir::new().unwrap();
        let v = idx(&dir);
        v.upsert(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        v.upsert(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();
        v.upsert(3, &[0.0, 0.0, 1.0, 0.0]).unwrap();
        assert_eq!(v.len(), 3);
        assert!(v.contains(1));

        // запрос ближе всего к ключу 1
        let hits = v.search(&[0.9, 0.1, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits[0].chunk_id, 1);

        // AC-Б4-2: повторный upsert того же ключа не растит индекс
        v.upsert(1, &[0.5, 0.5, 0.0, 0.0]).unwrap();
        assert_eq!(v.len(), 3, "замена вектора не должна плодить дубли");
    }

    #[test]
    fn rejects_wrong_dimension() {
        // AC-Б5-1: вектор иной длины отклоняется.
        let dir = TempDir::new().unwrap();
        let v = idx(&dir);
        let err = v.upsert(1, &[1.0, 0.0]).unwrap_err();
        assert!(matches!(err, VectorError::DimMismatch { .. }), "{err}");
        assert!(v.search(&[1.0, 0.0], 1).is_err());
    }

    #[test]
    fn remove_purges_vector() {
        // AC-Б8-2: удаление чистит вектор (нет «призраков» в выдаче).
        let dir = TempDir::new().unwrap();
        let v = idx(&dir);
        v.upsert(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        v.upsert(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();
        v.remove(1).unwrap();
        assert!(!v.contains(1));
        assert_eq!(v.len(), 1);
        let hits = v.search(&[1.0, 0.0, 0.0, 0.0], 5).unwrap();
        assert!(
            hits.iter().all(|h| h.chunk_id != 1),
            "удалённый ключ не в выдаче"
        );
    }

    #[test]
    fn persists_across_open() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".nexus/vectors.usearch");
        {
            let v = VectorIndex::open(&path, DIM).unwrap();
            v.upsert(7, &[0.0, 0.0, 0.0, 1.0]).unwrap();
            v.save().unwrap();
        }
        let v2 = VectorIndex::open(&path, DIM).unwrap();
        assert_eq!(v2.len(), 1);
        assert!(v2.contains(7));
        assert_eq!(v2.search(&[0.0, 0.0, 0.0, 1.0], 1).unwrap()[0].chunk_id, 7);
    }

    // ── Канон reconcile_embedding_model (R-3d, §6.5/§8.5, CORE-2a #2) ────────────────────────────
    //
    // R-3d: решение владельца — «полная чистка» (superset прежних реплик). Прежний тест
    // `reconcile_resets_on_model_or_dim_change` пинил СТАРУЮ core-семантику (4 файла БЕЗ chunks) —
    // ассерты обновлены ОСОЗНАННО: смена модели теперь чистит и `chunks` (+FTS триггерами) и
    // помечает эпизоды. Путь «та же модель» закреплён отдельным тестом как СТРОГИЙ no-op —
    // пользовательские индексы не пересобираются на ровном месте.

    /// Сеет производные vault, зависящие от embedding-модели: chunk (files+chunks, size_bytes=1),
    /// эпизод с `embed_model='old-model'` (chat_sessions+chat_episodes) и stale-файлы всех четырёх
    /// usearch-индексов. Идемпотентен (повторный сев после чистки — для табличного теста; upsert
    /// files возвращает size_bytes=1, чтобы durable-маркер -1 проверялся в каждом раунде заново).
    async fn seed_derived(db: &crate::db::Database, root: &Path) {
        db.writer()
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO files(path,hash,created_at,updated_at,indexed_at,size_bytes) \
                       VALUES('A.md','h',0,0,0,1) \
                       ON CONFLICT(path) DO UPDATE SET size_bytes=1; \
                     INSERT INTO chunks(file_id,chunk_index,content,char_start,char_end,token_count) \
                       SELECT id,0,'text',0,4,1 FROM files WHERE path='A.md'; \
                     INSERT OR IGNORE INTO chat_sessions(id,title,created_at,updated_at) VALUES(1,'s',0,0); \
                     INSERT INTO chat_episodes(session_id,summary,msg_count,last_msg_id,started_at,ended_at,embed_model,generated_at) \
                       VALUES(1,'sum',1,1,0,0,'old-model',0) \
                       ON CONFLICT(session_id) DO UPDATE SET embed_model='old-model';",
                )
            })
            .await
            .unwrap();
        for f in VECTOR_INDEX_FILES {
            std::fs::write(root.join(".nexus").join(f), b"stale").unwrap();
        }
    }

    /// `files.size_bytes` посеянного A.md (durable-маркер переиндексации: -1 после чистки).
    async fn seeded_file_size(db: &crate::db::Database) -> i64 {
        db.reader()
            .query(|c| {
                c.query_row("SELECT size_bytes FROM files WHERE path='A.md'", [], |r| {
                    r.get(0)
                })
            })
            .await
            .unwrap()
    }

    async fn count_chunks(db: &crate::db::Database) -> i64 {
        db.reader()
            .query(|c| c.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0)))
            .await
            .unwrap()
    }

    async fn episode_embed_model(db: &crate::db::Database) -> Option<String> {
        db.reader()
            .query(|c| {
                c.query_row(
                    "SELECT embed_model FROM chat_episodes WHERE session_id=1",
                    [],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap()
    }

    async fn open_db(root: &Path) -> crate::db::Database {
        std::fs::create_dir_all(root.join(".nexus")).unwrap();
        crate::db::Database::open(root.join(".nexus/nexus.db"))
            .await
            .unwrap()
    }

    /// R-3d: первое включение (settings пусты) — инициализация БЕЗ сноса: settings записаны
    /// (второй вызов → false), reindex=true, а уже существующие производные НЕ тронуты.
    #[tokio::test]
    async fn reconcile_first_run_initializes_without_wipe() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let db = open_db(root).await;
        seed_derived(&db, root).await;

        assert!(
            reconcile_embedding_model(&db, root, "bge-m3", 1024)
                .await
                .unwrap(),
            "первое включение требует индексации"
        );
        assert_eq!(count_chunks(&db).await, 1, "первый запуск не чистит chunks");
        for f in VECTOR_INDEX_FILES {
            assert!(
                root.join(".nexus").join(f).exists(),
                "первый запуск не сносит {f}"
            );
        }
        assert!(
            !reconcile_embedding_model(&db, root, "bge-m3", 1024)
                .await
                .unwrap(),
            "settings персистированы: повторный вызов той же моделью — false"
        );
    }

    /// R-3d (критично для данных пользователей): та же модель/dim → СТРОГИЙ no-op — false,
    /// chunks целы, все 4 файла индексов целы (байт-в-байт), embed_model эпизодов не тронут.
    /// Обычный старт БЕЗ смены модели не пересобирает пользовательские индексы.
    #[tokio::test]
    async fn reconcile_same_model_is_strict_noop() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let db = open_db(root).await;
        reconcile_embedding_model(&db, root, "bge-m3", 1024)
            .await
            .unwrap();
        seed_derived(&db, root).await;

        assert!(
            !reconcile_embedding_model(&db, root, "bge-m3", 1024)
                .await
                .unwrap(),
            "та же модель/dim — без переиндексации"
        );
        assert_eq!(count_chunks(&db).await, 1, "no-op: chunks не тронуты");
        assert_eq!(
            seeded_file_size(&db).await,
            1,
            "no-op: mtime+size-шорткат НЕ сломан (индексы не пересобираются на ровном месте)"
        );
        assert_eq!(
            episode_embed_model(&db).await.as_deref(),
            Some("old-model"),
            "no-op: эпизоды не помечены на переэмбеддинг"
        );
        for f in VECTOR_INDEX_FILES {
            assert_eq!(
                std::fs::read(root.join(".nexus").join(f)).unwrap(),
                b"stale",
                "no-op: файл {f} цел байт-в-байт"
            );
        }
    }

    /// R-3d: решение владельца §8.5 — смена модели И смена dim делают ПОЛНУЮ чистку производных:
    /// chunks пусты (+FTS триггерами), ВСЕ 4 usearch-файла снесены (вкл. chat_vectors — прежний
    /// desktop-путь его НЕ трогал), эпизоды помечены на переэмбеддинг, durable-маркер
    /// (size_bytes=-1) взведён, возврат reindex=true.
    #[tokio::test]
    async fn reconcile_model_or_dim_change_full_cleanup() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let db = open_db(root).await;
        reconcile_embedding_model(&db, root, "bge-m3", 1024)
            .await
            .unwrap();

        // Таблица: (метка, новая модель, новый dim) — смена модели, затем смена только dim.
        for (case, model, dim) in [
            ("смена модели", "other-model", 1024_usize),
            ("смена dim", "other-model", 768),
        ] {
            seed_derived(&db, root).await;
            assert!(
                reconcile_embedding_model(&db, root, model, dim)
                    .await
                    .unwrap(),
                "{case}: требует переиндексации"
            );
            assert_eq!(count_chunks(&db).await, 0, "{case}: chunks вычищены");
            assert_eq!(
                seeded_file_size(&db).await,
                -1,
                "{case}: durable-маркер — mtime+size-шорткат сломан, следующий скан перечанкует"
            );
            assert_eq!(
                episode_embed_model(&db).await,
                None,
                "{case}: эпизоды помечены на переэмбеддинг"
            );
            for f in VECTOR_INDEX_FILES {
                assert!(!root.join(".nexus").join(f).exists(), "{case}: {f} снесён");
            }
        }
    }
}
