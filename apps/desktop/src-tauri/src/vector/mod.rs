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
}
