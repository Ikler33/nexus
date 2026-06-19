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

/// Гард совместимости on-disk индексов с активной embedding-моделью/размерностью (CORE-2a follow-up #2).
///
/// Десктоп делает это в `open_vault` (app-приватный `reconcile_embedding_model`): при первом включении
/// RAG или смене модели/dim старые векторы несовместимы (другая семантика/размерность) → их надо
/// сбросить, иначе запрос НОВОЙ моделью против СТАРОГО индекса даёт `DimMismatch` (или семантический
/// мусор = ложная память). headless-agentd должен пройти ту же сверку ДО открытия индексов, иначе
/// унаследует пред-существующий `.nexus/*.usearch` (записанный прошлым прогоном десктопа под другой
/// моделью) и упадёт на первом search/upsert.
///
/// Логика (зеркало app, но СИММЕТРИЧНО по ВСЕМ четырём индексам — agentd читает память тем же
/// эмбеддером): сверяет `settings.embedding.{model,dim}` с активными `(model, dim)`. Совпали →
/// `false` (ничего не делаем). Иначе — на СМЕНЕ модели (prev_model задан) сносит ВСЕ файлы
/// [`VECTOR_INDEX_FILES`] (их перезаполнит индексатор/бэкфилл новой моделью) и сбрасывает
/// `chat_episodes.embed_model` (помечает эпизоды на переэмбеддинг); затем персистит новые
/// `settings` и возвращает `true` (нужна (пере)индексация). На ПЕРВОМ включении (prev_model нет)
/// файлов ещё нет — только пишем settings, `true`.
///
/// Возвращает `Ok(reindex_needed)`. Ошибки БД — `Err` (вызывающий деградирует: RAG/память off).
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
        // Модель/dim сменились → ВСЕ on-disk векторы несовместимы. Сносим файлы (перезаполнятся
        // новой моделью индексатором/бэкфиллом). `chunks` НЕ трогаем здесь (note-RAG переиндексация —
        // дело индексатора; agentd-skeleton его не гоняет, но дроп файла vectors.usearch достаточно,
        // чтобы не словить DimMismatch). Помечаем эпизоды на переэмбеддинг (summary-текст цел).
        for f in VECTOR_INDEX_FILES {
            let _ = std::fs::remove_file(root.join(".nexus").join(f));
        }
        db.writer()
            .call(|c| {
                c.execute("UPDATE chat_episodes SET embed_model=NULL", [])
                    .map(|_| ())
            })
            .await?;
        tracing::info!(from = ?prev_model, to = %model, dim, "agentd: смена embedding-модели → сброс векторных индексов (CORE-2a #2)");
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

    /// CORE-2a #2: reconcile_embedding_model. Первое включение (нет settings) → true, persisted.
    /// Та же модель/dim → false (no-op). Смена модели → сносит ВСЕ индекс-файлы + true; смена dim
    /// (та же модель) → тоже сброс + true.
    #[tokio::test]
    async fn reconcile_resets_on_model_or_dim_change() {
        use crate::db::Database;

        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".nexus")).unwrap();
        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();

        // Первое включение: settings пусты → reindex_needed=true, settings записаны.
        assert!(
            reconcile_embedding_model(&db, root, "bge-m3", 1024)
                .await
                .unwrap(),
            "первое включение требует индексации"
        );
        // Та же модель/dim → false (совместимо, ничего не делаем).
        assert!(
            !reconcile_embedding_model(&db, root, "bge-m3", 1024)
                .await
                .unwrap(),
            "та же модель/dim — no-op"
        );

        // Кладём stale файлы всех индексов.
        for f in VECTOR_INDEX_FILES {
            std::fs::write(root.join(".nexus").join(f), b"stale").unwrap();
        }
        // Смена МОДЕЛИ → сброс всех файлов + true.
        assert!(
            reconcile_embedding_model(&db, root, "other-model", 1024)
                .await
                .unwrap(),
            "смена модели требует переиндексации"
        );
        for f in VECTOR_INDEX_FILES {
            assert!(
                !root.join(".nexus").join(f).exists(),
                "stale {f} снесён при смене модели"
            );
        }

        // Снова stale; смена DIM (та же модель) → тоже сброс + true.
        for f in VECTOR_INDEX_FILES {
            std::fs::write(root.join(".nexus").join(f), b"stale").unwrap();
        }
        assert!(
            reconcile_embedding_model(&db, root, "other-model", 768)
                .await
                .unwrap(),
            "смена dim требует переиндексации"
        );
        for f in VECTOR_INDEX_FILES {
            assert!(
                !root.join(".nexus").join(f).exists(),
                "stale {f} снесён при смене dim"
            );
        }
    }
}
