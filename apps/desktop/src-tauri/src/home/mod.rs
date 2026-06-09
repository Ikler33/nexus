//! HOME-дашборд — бэкенд (визуал собирается отдельно по дизайну). H1: агрегация СТАТИЧЕСКИХ/
//! ДИНАМИЧЕСКИХ виджетов без LLM и без кэша — чистый SQL, мгновенно (концепт `PKM_Home_Concepts.md`,
//! зоны 2–3): статистика базы, недавние заметки, прогресс целей. H2 ([`widgets`]) — кэш LLM-виджетов
//! + refresh-режимы поверх планировщика ADR-007. Конкретные LLM-виджеты (daily brief / stale radar / …)
//! — отдельными срезами H3+ (см. `docs/dev/HOME_BACKEND_PLAN.md`).

pub mod stale;
pub mod widgets;

use serde::Serialize;

use crate::db::{DbResult, ReadPool};
use crate::goals::{self, Goal};
use crate::vault::NoteRef;

/// Сколько недавних заметок отдаём в виджет «Недавние файлы» (зона 2).
const RECENT_LIMIT: i64 = 8;

/// Счётчики базы — статический виджет «Статистика базы» (зона 3).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeStats {
    pub notes: i64,
    pub tags: i64,
    pub links: i64,
    pub words: i64,
}

/// Данные HOME для статических/динамических зон (H1). LLM-виджеты приходят отдельно (H2+).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeData {
    pub stats: HomeStats,
    pub recent: Vec<NoteRef>,
    pub goals: Vec<Goal>,
}

/// Собирает статические/динамические данные HOME (без LLM). Чистый read — офлайн, без сети.
pub async fn home_data(reader: &ReadPool) -> DbResult<HomeData> {
    let stats = reader
        .query(|c| {
            let notes: i64 =
                c.query_row("SELECT count(*) FROM files WHERE is_deleted=0", [], |r| {
                    r.get(0)
                })?;
            let tags: i64 = c.query_row("SELECT count(*) FROM tags", [], |r| r.get(0))?;
            let links: i64 = c.query_row("SELECT count(*) FROM links", [], |r| r.get(0))?;
            let words: i64 = c.query_row(
                "SELECT COALESCE(SUM(word_count),0) FROM files WHERE is_deleted=0",
                [],
                |r| r.get(0),
            )?;
            Ok(HomeStats {
                notes,
                tags,
                links,
                words,
            })
        })
        .await?;

    let recent = reader
        .query(|c| {
            let mut stmt = c.prepare(
                "SELECT path, title FROM files WHERE is_deleted=0 ORDER BY updated_at DESC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([RECENT_LIMIT], |r| {
                    Ok(NoteRef {
                        path: r.get(0)?,
                        title: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;

    let goals = goals::list_goals(reader).await?;
    Ok(HomeData {
        stats,
        recent,
        goals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::TempDir;

    async fn open(root: &std::path::Path) -> Database {
        Database::open(root.join(".nexus/nexus.db")).await.unwrap()
    }

    #[tokio::test]
    async fn home_data_aggregates_stats_recent_goals() {
        let dir = TempDir::new().unwrap();
        let db = open(dir.path()).await;
        db.writer()
            .call(|c| {
                // Две заметки (одна — цель с progress), тег, связь.
                c.execute(
                    "INSERT INTO files (path,hash,title,created_at,updated_at,indexed_at,size_bytes,word_count) \
                     VALUES ('A.md','h','A',0,10,0,1,5),('Goal.md','h2','G',0,20,0,1,7)",
                    [],
                )?;
                c.execute("INSERT INTO tags (name) VALUES ('project')", [])?;
                let fid: i64 =
                    c.query_row("SELECT id FROM files WHERE path='A.md'", [], |r| r.get(0))?;
                let gid: i64 =
                    c.query_row("SELECT id FROM files WHERE path='Goal.md'", [], |r| r.get(0))?;
                c.execute(
                    "INSERT INTO links (source_id,target_id,target_raw,link_type,line_number) \
                     VALUES (?1,?2,'Goal','wiki',1)",
                    rusqlite::params![fid, gid],
                )?;
                // Цель: инлайн-тег #goal + frontmatter progress.
                c.execute(
                    "INSERT INTO file_tags (file_id,tag_id) SELECT ?1, id FROM tags WHERE name='project'",
                    [gid],
                )?;
                c.execute("INSERT OR IGNORE INTO tags (name) VALUES ('goal')", [])?;
                c.execute(
                    "INSERT INTO file_tags (file_id,tag_id) SELECT ?1, id FROM tags WHERE name='goal'",
                    [gid],
                )?;
                c.execute(
                    "INSERT INTO frontmatter_fields (file_id,key,value) VALUES (?1,'progress','0.4')",
                    [gid],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let data = home_data(db.reader()).await.unwrap();
        assert_eq!(data.stats.notes, 2);
        assert_eq!(data.stats.links, 1);
        assert_eq!(data.stats.words, 12, "5+7 слов");
        assert!(data.stats.tags >= 1);
        // Недавние — сначала Goal.md (updated_at=20), потом A.md (10).
        assert_eq!(
            data.recent.first().map(|n| n.path.as_str()),
            Some("Goal.md")
        );
        // Цель распознана с прогрессом 40.
        assert_eq!(data.goals.len(), 1);
        assert_eq!(data.goals[0].progress, Some(40));
    }
}
