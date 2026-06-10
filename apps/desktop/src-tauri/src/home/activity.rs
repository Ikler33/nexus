//! H6 (DP-1): данные «Активности» HOME — heatmap правок, серия дней, изменения сегодня,
//! заметки-сироты и «Продолжить» (последняя правленая заметка). Всё выводится из ТЕКУЩИХ
//! `files.updated_at` — истории правок в БД нет, поэтому день считается по последней правке
//! файла (файл, правленный в пн и пт, даёт только пт). Честное ограничение — см. BACKLOG.
//!
//! Часовой пояс приходит с фронта (`tz_offset_min`, как `Date.getTimezoneOffset()` со знаком
//! «минуты ЗАПАДНЕЕ UTC») — «сегодня»/серии считаются в локальных днях пользователя.

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::db::{DbResult, ReadPool};

/// Окно heatmap: 17 недель × 7 дней (как в макете).
pub const HEAT_WEEKS: i64 = 17;
const HEAT_DAYS: i64 = HEAT_WEEKS * 7;

/// День heatmap: смещение от сегодня (0 = сегодня, 1 = вчера, …) + число правленых файлов.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatDay {
    pub days_ago: i64,
    pub count: i64,
}

/// «Продолжить»: последняя правленая заметка (сниппет дочитывает команда — у неё есть root).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueNote {
    pub path: String,
    pub title: Option<String>,
    pub updated_at: i64,
    pub words: i64,
    /// Первые строки тела (без frontmatter/заголовка); заполняет команда чтением с диска.
    pub snippet: String,
}

/// Данные зоны «Активность» (H6). Тренд недели — сравнение с предыдущей (есть в mtime-данных).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityData {
    /// Дни с правками за окно heatmap (нулевые дни не передаются — фронт строит сетку сам).
    pub heatmap: Vec<HeatDay>,
    pub changes_today: i64,
    /// Файлов правлено за последние 7 дней / за предыдущие 7 (тренд недели).
    pub week: i64,
    pub prev_week: i64,
    /// Текущая серия дней с правками (считая сегодня или вчера, если сегодня пусто).
    pub streak_days: i64,
    /// Лучшая серия В ОКНЕ heatmap (всей истории в БД нет).
    pub best_streak: i64,
    /// Заметки без входящих ссылок.
    pub orphans: i64,
    #[serde(rename = "continue")]
    pub continue_note: Option<ContinueNote>,
}

/// Локальный день (число дней от эпохи) для unix-времени и смещения таймзоны.
fn local_day(secs: i64, tz_offset_min: i64) -> i64 {
    (secs - tz_offset_min * 60).div_euclid(86_400)
}

/// Собирает активность из `files.updated_at` (чистый read, без сети).
pub async fn activity_data(
    reader: &ReadPool,
    now: i64,
    tz_offset_min: i64,
) -> DbResult<ActivityData> {
    let today = local_day(now, tz_offset_min);
    let since = (today - HEAT_DAYS + 1) * 86_400 + tz_offset_min * 60;

    let (day_counts, orphans, cont) = reader
        .query(move |c| {
            // Правки по локальным дням окна.
            let mut stmt = c.prepare(
                "SELECT (updated_at - ?1*60) / 86400 AS d, count(*) FROM files \
                 WHERE is_deleted=0 AND updated_at >= ?2 GROUP BY d",
            )?;
            let day_counts: Vec<(i64, i64)> = stmt
                .query_map([tz_offset_min, since], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let orphans: i64 = c.query_row(
                "SELECT count(*) FROM files WHERE is_deleted=0 \
                 AND id NOT IN (SELECT DISTINCT target_id FROM links WHERE target_id IS NOT NULL)",
                [],
                |r| r.get(0),
            )?;

            let cont = c
                .query_row(
                    "SELECT path, title, updated_at, word_count FROM files \
                     WHERE is_deleted=0 ORDER BY updated_at DESC LIMIT 1",
                    [],
                    |r| {
                        Ok(ContinueNote {
                            path: r.get(0)?,
                            title: r.get(1)?,
                            updated_at: r.get(2)?,
                            words: r.get(3)?,
                            snippet: String::new(),
                        })
                    },
                )
                .optional()?;
            Ok((day_counts, orphans, cont))
        })
        .await?;

    let mut heatmap = Vec::with_capacity(day_counts.len());
    let mut active = vec![false; HEAT_DAYS as usize];
    let mut changes_today = 0;
    let (mut week, mut prev_week) = (0, 0);
    for (day, count) in day_counts {
        let ago = today - day;
        if !(0..HEAT_DAYS).contains(&ago) {
            continue; // правки «из будущего» (рассинхрон часов) не ломают сетку
        }
        heatmap.push(HeatDay {
            days_ago: ago,
            count,
        });
        active[ago as usize] = true;
        if ago == 0 {
            changes_today = count;
        }
        if ago < 7 {
            week += count;
        } else if ago < 14 {
            prev_week += count;
        }
    }

    // Текущая серия: с сегодня (или со вчера, если сегодня пока пусто).
    let start = usize::from(!active.first().copied().unwrap_or(false));
    let streak_days = active[start..].iter().take_while(|d| **d).count() as i64;
    // Лучшая серия в окне.
    let mut best = 0i64;
    let mut run = 0i64;
    for day in &active {
        run = if *day { run + 1 } else { 0 };
        best = best.max(run);
    }

    Ok(ActivityData {
        heatmap,
        changes_today,
        week,
        prev_week,
        streak_days,
        best_streak: best.max(streak_days),
        orphans,
        continue_note: cont,
    })
}

/// Сниппет «Продолжить»: первые строки тела без frontmatter и первого заголовка, ≤`max` симв.
pub fn continue_snippet(body: &str, max: usize) -> String {
    let mut text = body;
    // Срез frontmatter.
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            text = &rest[end + 4..];
        }
    }
    let cleaned = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out: String = cleaned.chars().take(max).collect();
    if cleaned.chars().count() > max {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::TempDir;

    const DAY: i64 = 86_400;

    async fn db_with_files(rows: &[(&str, i64)]) -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("nexus.db")).await.unwrap();
        let rows: Vec<(String, i64)> = rows.iter().map(|(p, t)| (p.to_string(), *t)).collect();
        db.writer()
            .call(move |c| {
                for (path, mtime) in &rows {
                    c.execute(
                        "INSERT INTO files (path,hash,title,created_at,updated_at,indexed_at,size_bytes,word_count) \
                         VALUES (?1,'h',?1,0,?2,0,1,42)",
                        rusqlite::params![path, mtime],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();
        (dir, db)
    }

    /// H6: heatmap по локальным дням, серия с учётом «вчера», тренд недель, continue — самая свежая.
    #[tokio::test]
    async fn activity_buckets_streak_and_continue() {
        let now = 1_780_000_000; // 2026-05-28, UTC
        let (_d, db) = db_with_files(&[
            ("today.md", now - 100),
            ("today2.md", now - 200),
            ("yesterday.md", now - DAY),
            ("two-back.md", now - 2 * DAY),
            ("week-old.md", now - 8 * DAY),  // прошлая неделя
            ("ancient.md", now - 300 * DAY), // вне окна — не в heatmap
        ])
        .await;

        let a = activity_data(db.reader(), now, 0).await.unwrap();
        assert_eq!(a.changes_today, 2);
        assert_eq!(a.streak_days, 3, "сегодня+вчера+позавчера");
        assert_eq!(a.week, 4, "за 7 дней");
        assert_eq!(a.prev_week, 1, "8 дней назад");
        assert!(a.best_streak >= 3);
        assert_eq!(
            a.heatmap.iter().map(|h| h.count).sum::<i64>(),
            5,
            "ancient вне окна"
        );
        assert_eq!(
            a.continue_note.as_ref().map(|c| c.path.as_str()),
            Some("today.md")
        );
        // Все заметки — сироты (links пуст).
        assert_eq!(a.orphans, 6);
    }

    /// Таймзона двигает границу «сегодня»: правка в 23:30 UTC при UTC+3 — уже «завтрашний» день.
    #[tokio::test]
    async fn timezone_shifts_day_boundary() {
        let midnight_utc = 1_780_000_000 / DAY * DAY; // 00:00 UTC
        let late_evening = midnight_utc - 1800; // 23:30 предыдущего дня UTC
        let (_d, db) = db_with_files(&[("note.md", late_evening)]).await;

        // UTC: правка вчера → сегодня пусто.
        let utc = activity_data(db.reader(), midnight_utc + 3600, 0)
            .await
            .unwrap();
        assert_eq!(utc.changes_today, 0);
        assert_eq!(utc.streak_days, 1, "вчерашняя серия жива");

        // UTC+3 (offset −180 мин): 23:30 UTC = 02:30 локально → «сегодня».
        let local = activity_data(db.reader(), midnight_utc + 3600, -180)
            .await
            .unwrap();
        assert_eq!(local.changes_today, 1);
    }

    /// Сниппет: frontmatter и заголовки срезаны, обрезка по символам с многоточием.
    #[test]
    fn snippet_strips_frontmatter_and_headings() {
        let body = "---\ntitle: X\n---\n# Заголовок\n\nПервый абзац текста.\nВторая строка.";
        assert_eq!(
            continue_snippet(body, 100),
            "Первый абзац текста. Вторая строка."
        );
        let cut = continue_snippet(body, 10);
        assert_eq!(cut.chars().count(), 11, "10 + …");
        assert!(cut.ends_with('…'));
    }
}
