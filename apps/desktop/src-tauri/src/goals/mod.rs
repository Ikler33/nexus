//! «Прогресс целей» (#35, vision-волна 2): кросс-файловый список заметок-целей (маркер — инлайн-тег
//! `#goal`) с прогрессом из frontmatter-поля `progress`. Чистый SQL-read из индекса (tags/file_tags +
//! frontmatter_fields), без LLM/сети — детерминирован и работает офлайн (AC-GP-4 / AC-X-1).
//!
//! Маркер v1 — ИНЛАЙН `#goal` в теле (парсер извлекает body-теги; frontmatter `tags: [goal]` пока НЕ
//! даёт file_tag — см. BACKLOG). Шкала прогресса 0–100 (D6): `0≤x≤1`→×100, хвостовой `%` срезается;
//! отсутствие / нечисловое / вне диапазона → `None` («нет прогресса», D7 — НЕ тихий 0%).

use serde::Serialize;

use crate::db::{DbResult, ReadPool};

/// Маркер-тег цели (хранится в `tags.name` в нижнем регистре, без `#`).
const GOAL_TAG: &str = "goal";

/// Цель: путь, заголовок и прогресс 0–100 (`None` — нет валидного значения, D7).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub path: String,
    pub title: Option<String>,
    pub progress: Option<u8>,
}

/// Парс прогресса из сырого frontmatter-значения (D6/D7): срез хвостового `%`, `0≤x≤1`→×100,
/// валидация диапазона 0–100. Возвращает `None` для отсутствующего/нечислового/вне диапазона.
fn parse_progress(raw: &str) -> Option<u8> {
    let s = raw.trim().trim_end_matches('%').trim();
    let n: f64 = s.parse().ok()?;
    if !n.is_finite() {
        return None;
    }
    // 0≤x≤1 трактуется как доля (×100); >1 — уже проценты.
    let pct = if (0.0..=1.0).contains(&n) {
        n * 100.0
    } else {
        n
    };
    if (0.0..=100.0).contains(&pct) {
        Some(pct.round() as u8)
    } else {
        None
    }
}

/// Все заметки-цели (инлайн-тег `#goal`) с прогрессом. Сорт по пути (детерминизм). Пусто, если целей нет.
pub async fn list_goals(reader: &ReadPool) -> DbResult<Vec<Goal>> {
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT f.path, f.title, ff.value \
                 FROM files f \
                 JOIN file_tags ft ON ft.file_id = f.id \
                 JOIN tags t ON t.id = ft.tag_id \
                 LEFT JOIN frontmatter_fields ff ON ff.file_id = f.id AND ff.key = 'progress' \
                 WHERE f.is_deleted = 0 AND t.name = ?1 \
                 ORDER BY f.path",
            )?;
            let rows = stmt.query_map([GOAL_TAG], |r| {
                let path: String = r.get(0)?;
                let title: Option<String> = r.get(1)?;
                let raw: Option<String> = r.get(2)?;
                Ok(Goal {
                    path,
                    title,
                    progress: raw.as_deref().and_then(parse_progress),
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<Goal>>>()
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{EmbeddingProvider, MockEmbedder};
    use crate::db::Database;
    use crate::indexer::Indexer;
    use crate::vector::VectorIndex;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn index(root: &std::path::Path, files: &[(&str, &str)]) -> Database {
        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors.usearch"), 16).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
        let idx = Indexer::with_rag(&db, root.to_path_buf(), embedder, vectors, true);
        for (name, body) in files {
            fs::write(root.join(name), body).unwrap();
            idx.index_file(name).await.unwrap();
        }
        db
    }

    #[test]
    fn parse_progress_scale_and_validation() {
        assert_eq!(parse_progress("80"), Some(80)); // проценты как есть
        assert_eq!(parse_progress("80%"), Some(80)); // срез %
        assert_eq!(parse_progress(" 80 "), Some(80)); // trim
        assert_eq!(parse_progress("0.5"), Some(50)); // доля ×100
        assert_eq!(parse_progress("1"), Some(100)); // 0≤x≤1 → 100
        assert_eq!(parse_progress("0"), Some(0));
        assert_eq!(parse_progress("150"), None); // вне диапазона
        assert_eq!(parse_progress("WIP"), None); // нечисловое
        assert_eq!(parse_progress("-5"), None); // отрицательное
    }

    /// Цели по инлайн-тегу `#goal`; прогресс парсится; D7-политика (нет/битое → None); не-цель исключена.
    #[tokio::test]
    async fn lists_goals_with_progress_and_d7_policy() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = index(
            &root,
            &[
                ("g1.md", "---\nprogress: 80\n---\n# Goal 1\n\n#goal\n"),
                ("g2.md", "---\nprogress: 0.5\n---\n# Goal 2\n\n#goal\n"),
                ("g3.md", "# Goal 3\n\n#goal\n\nбез прогресса\n"), // нет progress → None
                ("g4.md", "---\nprogress: WIP\n---\n# Goal 4\n\n#goal\n"), // битый → None
                ("n1.md", "# Not a goal\n\nобычная заметка\n"),    // не цель
            ],
        )
        .await;

        let goals = list_goals(db.reader()).await.unwrap();
        let by: HashMap<&str, Option<u8>> = goals
            .iter()
            .map(|g| (g.path.as_str(), g.progress))
            .collect();
        assert_eq!(by.len(), 4, "4 цели (#goal), n1 исключена");
        assert_eq!(by["g1.md"], Some(80));
        assert_eq!(by["g2.md"], Some(50));
        assert_eq!(by["g3.md"], None, "нет прогресса → None (D7)");
        assert_eq!(by["g4.md"], None, "битое значение → None (D7)");
        assert!(!by.contains_key("n1.md"), "не-цель исключена");
    }
}
