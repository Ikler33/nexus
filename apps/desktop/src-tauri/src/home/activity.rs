//! H6 (DP-1): данные «Активности» HOME — heatmap правок, серия дней, изменения сегодня,
//! заметки-сироты и «Продолжить» (последняя правленая заметка).
//!
//! ACT-1 (P5): heatmap/серия/тренд считаются по `edit_events` (P2) — ЧЕСТНАЯ история правок:
//! файл, правленный в пн и пт, даёт ОБА дня (в отличие от `files.updated_at`, дававшего только
//! последний). Для периода ДО того, как Nexus начал отслеживать правки (раньше самого раннего
//! события — там лишь bootstrap-создания при первичной индексации), фолбэк на `files.updated_at`,
//! чтобы heatmap существующего vault не схлопывался в один день индексации. `orphans`/`continue`
//! — текущее состояние (не историчны), остаются на `files`.
//!
//! Часовой пояс приходит с фронта (`tz_offset_min`, как `Date.getTimezoneOffset()` со знаком
//! «минуты ЗАПАДНЕЕ UTC») — «сегодня»/серии считаются в локальных днях пользователя.

use std::collections::HashMap;

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

/// Данные зоны «Активность» (H6). Тренд недели — сравнение текущих 7 дней с предыдущими 7.
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

/// Собирает активность: heatmap/серия/тренд — из `edit_events` с mtime-фолбэком (ACT-1),
/// сироты/«Продолжить» — из `files`. Чистый read, без сети.
pub async fn activity_data(
    reader: &ReadPool,
    now: i64,
    tz_offset_min: i64,
) -> DbResult<ActivityData> {
    let today = local_day(now, tz_offset_min);
    let since = (today - HEAT_DAYS + 1) * 86_400 + tz_offset_min * 60;

    let (mtime_counts, evt_counts, evt_start, orphans, cont) = reader
        .query(move |c| {
            // Фолбэк по mtime (последняя правка файла) — для истории ДО отслеживания событий.
            let mut stmt = c.prepare(
                "SELECT (updated_at - ?1*60) / 86400 AS d, count(*) FROM files \
                 WHERE is_deleted=0 AND updated_at >= ?2 GROUP BY d",
            )?;
            let mtime_counts: Vec<(i64, i64)> = stmt
                .query_map([tz_offset_min, since], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // Честная история правок (ACT-1): число событий-правок по локальным дням окна. Удаления
            // не считаем активностью письма (kind IN create|modify). count(*) = правки (не файлы):
            // файл, правленный дважды за день, даёт 2 — гранулярнее mtime.
            let mut estmt = c.prepare(
                "SELECT (ts - ?1*60) / 86400 AS d, count(*) FROM edit_events \
                 WHERE ts >= ?2 AND kind IN ('create','modify') GROUP BY d",
            )?;
            let evt_counts: Vec<(i64, i64)> = estmt
                .query_map([tz_offset_min, since], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // Локальный день самого раннего события = граница «начали отслеживать». До неё (включая
            // её — там bootstrap-создания) берём mtime; после — события. NULL → событий нет вовсе.
            let evt_start: Option<i64> = c.query_row(
                "SELECT (MIN(ts) - ?1*60) / 86400 FROM edit_events",
                [tz_offset_min],
                |r| r.get(0),
            )?;

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
            Ok((mtime_counts, evt_counts, evt_start, orphans, cont))
        })
        .await?;

    // Слияние «честная история + фолбэк». Граница `start` = локальный день самого раннего события
    // (= когда Nexus начал отслеживать). Дни ПОСЛЕ неё — по событиям (пустой день = честный ноль),
    // день начала и раньше — по mtime (там лишь bootstrap-создания первичной индексации).
    //   Условие `!evt_counts.is_empty()`: если в ОКНЕ событий нет (vault не трогали в Nexus, или все
    // события старее окна), весь heatmap берём по mtime — иначе при `start` левее окна обе зоны
    // оказались бы пустыми и heatmap схлопнулся бы в ноль (регресс против до-ACT-1). С событиями в
    // окне `start` (даже левее окна) даёт всё окно по событиям — пустые дни честно нулевые.
    let day_counts: Vec<(i64, i64)> = match evt_start {
        Some(start) if !evt_counts.is_empty() => {
            let mut by_day: HashMap<i64, i64> = HashMap::new();
            for (d, ct) in mtime_counts {
                if d <= start {
                    by_day.insert(d, ct);
                }
            }
            for (d, ct) in evt_counts {
                if d > start {
                    by_day.insert(d, ct);
                }
            }
            by_day.into_iter().collect()
        }
        _ => mtime_counts, // нет событий в окне (или вообще) → целиком mtime, не обнуляем heatmap
    };

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

    /// ACT-1: heatmap по edit_events даёт ИСТИННУЮ мультидневную историю (файл, правленный в
    /// разные дни, даёт все дни — не только последний по mtime), а для периода ДО первого события
    /// (раньше начала отслеживания) — фолбэк на mtime, чтобы старые заметки не пропали.
    #[tokio::test]
    async fn activity_uses_edit_events_with_mtime_fallback() {
        let now = 1_780_000_000; // 2026-05-28, UTC
                                 // note.md: mtime=сегодня (по mtime дал бы только сегодня); old.md: mtime=40 дней назад.
        let (_d, db) = db_with_files(&[("note.md", now), ("old.md", now - 40 * DAY)]).await;
        db.writer()
            .call(move |c| {
                let id: i64 =
                    c.query_row("SELECT id FROM files WHERE path='note.md'", [], |r| {
                        r.get(0)
                    })?;
                // Якорь начала отслеживания — create 30 дней назад (этот день и раньше → mtime-зона).
                // Затем реальные правки note.md: 2× сегодня, 1× вчера, 1× позавчера (всё ПОСЛЕ якоря).
                for (ts, kind) in [
                    (now - 30 * DAY, "create"),
                    (now, "modify"),
                    (now, "modify"),
                    (now - DAY, "modify"),
                    (now - 2 * DAY, "modify"),
                ] {
                    c.execute(
                        "INSERT INTO edit_events (file_id,ts,kind,words_delta,words_after) \
                         VALUES (?1,?2,?3,0,42)",
                        rusqlite::params![id, ts, kind],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        let a = activity_data(db.reader(), now, 0).await.unwrap();
        // Истинная история: сегодня=2 правки (не 1 как по mtime), плюс вчера и позавчера присутствуют.
        assert_eq!(
            a.changes_today, 2,
            "две правки сегодня (count событий, не файлов)"
        );
        let by_ago: HashMap<i64, i64> = a.heatmap.iter().map(|h| (h.days_ago, h.count)).collect();
        assert_eq!(by_ago.get(&1), Some(&1), "вчера — из событий");
        assert_eq!(by_ago.get(&2), Some(&1), "позавчера — из событий");
        assert_eq!(a.streak_days, 3, "сегодня+вчера+позавчера подряд");
        assert_eq!(a.week, 4, "2+1+1 за 7 дней");
        // Фолбэк: old.md (mtime 40 дней назад, событий нет) — из mtime, т.к. это ДО якоря (30 дней).
        assert_eq!(by_ago.get(&40), Some(&1), "старая заметка из mtime-фолбэка");
    }

    /// Регресс (ревью ACT-1): события есть, но ВСЕ старее окна heatmap, а файл свежий по mtime.
    /// Раньше evt_start уезжал левее окна → обе зоны пусты → heatmap обнулялся. Теперь (нет событий
    /// В ОКНЕ) — фолбэк на mtime, активность видна.
    #[tokio::test]
    async fn activity_events_only_outside_window_falls_back_to_mtime() {
        let now = 1_780_000_000;
        // fresh.md правлен сегодня (в окне); anchor.md — 200 дней назад (вне окна 119 дней).
        let (_d, db) = db_with_files(&[("fresh.md", now), ("anchor.md", now - 200 * DAY)]).await;
        db.writer()
            .call(move |c| {
                let id: i64 =
                    c.query_row("SELECT id FROM files WHERE path='anchor.md'", [], |r| {
                        r.get(0)
                    })?;
                // Единственное событие — 200 дней назад (старее окна → evt_counts в окне пуст).
                c.execute(
                    "INSERT INTO edit_events (file_id,ts,kind,words_delta,words_after) \
                     VALUES (?1,?2,'create',0,42)",
                    rusqlite::params![id, now - 200 * DAY],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let a = activity_data(db.reader(), now, 0).await.unwrap();
        assert_eq!(
            a.changes_today, 1,
            "fresh.md из mtime-фолбэка, heatmap не пуст"
        );
        assert!(
            a.heatmap.iter().any(|h| h.days_ago == 0),
            "сегодняшняя активность видна"
        );
    }

    /// Без событий вовсе (edit_events пуст) — поведение как до ACT-1: целиком по mtime.
    #[tokio::test]
    async fn activity_no_events_falls_back_to_mtime() {
        let now = 1_780_000_000;
        let (_d, db) = db_with_files(&[("a.md", now), ("b.md", now - DAY)]).await;
        let a = activity_data(db.reader(), now, 0).await.unwrap();
        assert_eq!(a.changes_today, 1);
        assert_eq!(a.streak_days, 2);
        assert_eq!(a.heatmap.iter().map(|h| h.count).sum::<i64>(), 2);
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
