//! Персист ленты (NF-3, D6): `news_items` + `news_runs` в `nexus.db` (миграция 010).
//! Дедуп — `url UNIQUE` + `ON CONFLICT DO NOTHING` (AC-NF-4: повторный прогон не плодит дублей
//! и НЕ перетирает прочитанность); ретенция 30 дней по `fetched_at` (AC-NF-5, чистит GC-шаг
//! прогона); сводка/статы прогона — `news_runs` (видимые пропуски источников, no silent caps).

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{DbResult, ReadPool, WriteActor};

/// Ретенция ленты (D6): записи и прогоны старше — удаляются; «навсегда» = «в заметку».
pub const RETENTION_DAYS: i64 = 30;

/// Готовая к записи строка ленты (вход — из LLM-этапа; поля денормализованы под UI).
#[derive(Debug, Clone)]
pub struct NewRow {
    pub source_id: String,
    pub url: String,
    pub title: String,
    pub title_ru: String,
    pub summary_ru: String,
    pub topic: String,
    pub lang_ru: bool,
    pub published_at: i64,
    /// Ссылка на HN-обсуждение (NF-6 хвост) — `None` для не-HN / текстовых HN-постов.
    pub comments_url: Option<String>,
}

/// Запись ленты для UI (camelCase — контракт фронта, бриф `NEWS_FEED_BRIEF.md`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    pub id: i64,
    pub source_id: String,
    pub url: String,
    pub title_ru: String,
    pub summary_ru: String,
    pub topic: String,
    pub lang_ru: bool,
    pub published_at: i64,
    pub read: bool,
    /// Ссылка на обсуждение на HN (NF-6 хвост): ридер показывает кнопку «Обсуждение на HN» рядом
    /// с «Оригинал». `None` — не-HN / текстовый HN-пост (там `url` уже == обсуждение).
    pub comments_url: Option<String>,
}

/// B12: структурный сигнал «LLM-анализатор недоступен» (вместо RU-префикс-протокола в `errors[]`,
/// который фронт сниффил регексом). Персистится JSON'ом в nullable-колонке `news_runs.llm_down`
/// (миграция 027); человекочитаемая строка в `errors[]` остаётся (видимый список ошибок прогона).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmDownInfo {
    /// URL эндпоинта оценки, который был недоступен; `None` — эндпоинт ИИ не задан.
    pub endpoint: Option<String>,
    /// `true` — часть батчей прошла, лента обновлена частично; `false` — недоступен весь прогон
    /// (двухуровневость W-2: баннер фронта — только на тотальный сбой).
    pub partial: bool,
}

/// Итог прогона для шапки страницы (последняя запись `news_runs`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsRun {
    pub run_at: i64,
    pub digest_ru: String,
    pub items_new: i64,
    pub sources_ok: i64,
    pub sources_total: i64,
    pub llm_failed: i64,
    /// Видимые ошибки источников («источник: причина») — no silent caps (AC-NF-1).
    pub errors: Vec<String>,
    /// B12: сбой ВЫЗОВА LLM-оценки в этом прогоне; `None` — вызовы живы (или старая запись до
    /// миграции 027 — тогда фронт распознаёт legacy-строку в `errors`).
    pub llm_down: Option<LlmDownInfo>,
}

/// Вставляет оценённые записи; возвращает число НОВЫХ (дубликаты по url молча пропущены —
/// это и есть дедуп AC-NF-4, существующие строки не трогаются → read_at цел).
pub async fn insert_items(
    writer: &WriteActor,
    rows: Vec<NewRow>,
    fetched_at: i64,
) -> DbResult<usize> {
    writer
        .call(move |c| {
            let tx = c.transaction()?;
            let mut inserted = 0usize;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO news_items(source_id,url,title,title_ru,summary_ru,topic,lang_ru,\
                     published_at,fetched_at,comments_url) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) \
                     ON CONFLICT(url) DO NOTHING",
                )?;
                for r in &rows {
                    inserted += stmt.execute(params![
                        r.source_id,
                        r.url,
                        r.title,
                        r.title_ru,
                        r.summary_ru,
                        r.topic,
                        r.lang_ru,
                        r.published_at,
                        fetched_at,
                        r.comments_url
                    ])?;
                }
            }
            tx.commit()?;
            Ok(inserted)
        })
        .await
}

/// Лента для UI: свежие сверху, скрытые — никогда; фильтры по теме/непрочитанному; страница
/// `limit`+`offset` (кросс-план #22-урок: без безлимитных выгрузок).
pub async fn list_items(
    reader: &ReadPool,
    topic: Option<String>,
    unread_only: bool,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<NewsItem>> {
    reader
        .query(move |c| {
            let mut sql = String::from(
                "SELECT id,source_id,url,title_ru,summary_ru,topic,lang_ru,published_at,\
                 read_at IS NOT NULL,comments_url FROM news_items WHERE hidden=0",
            );
            let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            if let Some(t) = topic {
                sql.push_str(" AND topic=?");
                args.push(Box::new(t));
            }
            if unread_only {
                sql.push_str(" AND read_at IS NULL");
            }
            sql.push_str(" ORDER BY published_at DESC, id DESC LIMIT ? OFFSET ?");
            args.push(Box::new(limit));
            args.push(Box::new(offset));
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(args.iter().map(|a| a.as_ref())),
                    |r| {
                        Ok(NewsItem {
                            id: r.get(0)?,
                            source_id: r.get(1)?,
                            url: r.get(2)?,
                            title_ru: r.get(3)?,
                            summary_ru: r.get(4)?,
                            topic: r.get(5)?,
                            lang_ru: r.get(6)?,
                            published_at: r.get(7)?,
                            read: r.get(8)?,
                            comments_url: r.get(9)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Темы непрочитанной части ленты (чипы-фильтры, по убыванию частоты).
pub async fn list_topics(reader: &ReadPool) -> DbResult<Vec<String>> {
    reader
        .query(|c| {
            let mut stmt = c.prepare(
                "SELECT topic FROM news_items WHERE hidden=0 GROUP BY topic ORDER BY count(*) DESC",
            )?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Из `urls` оставляет ТОЛЬКО ещё не виденные лентой (префильтр ДО LLM-этапа: не жечь модель
/// на уже сохранённых записях; сам insert дополнительно защищён `ON CONFLICT`). `IN`-чанки по
/// 500 — guard лимита SQLite-переменных (урок V2.3).
pub async fn filter_new_urls(
    reader: &ReadPool,
    urls: Vec<String>,
) -> DbResult<std::collections::HashSet<String>> {
    reader
        .query(move |c| {
            let mut existing = std::collections::HashSet::new();
            for chunk in urls.chunks(500) {
                let placeholders = vec!["?"; chunk.len()].join(",");
                let sql = format!("SELECT url FROM news_items WHERE url IN ({placeholders})");
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt
                    .query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                        r.get::<_, String>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                existing.extend(rows);
            }
            Ok(urls.into_iter().filter(|u| !existing.contains(u)).collect())
        })
        .await
}

/// Отметка прочитано/непрочитано (AC-NF-9).
pub async fn mark_read(writer: &WriteActor, id: i64, read: bool, now: i64) -> DbResult<()> {
    writer
        .call(move |c| {
            let read_at: Option<i64> = read.then_some(now);
            c.execute(
                "UPDATE news_items SET read_at=?1 WHERE id=?2",
                params![read_at, id],
            )?;
            Ok(())
        })
        .await
}

/// Запись ленты по id (для «в заметку»).
pub async fn get_item(reader: &ReadPool, id: i64) -> DbResult<Option<NewsItem>> {
    reader
        .query(move |c| {
            c.query_row(
                "SELECT id,source_id,url,title_ru,summary_ru,topic,lang_ru,published_at,\
                 read_at IS NOT NULL,comments_url FROM news_items WHERE id=?1",
                [id],
                |r| {
                    Ok(NewsItem {
                        id: r.get(0)?,
                        source_id: r.get(1)?,
                        url: r.get(2)?,
                        title_ru: r.get(3)?,
                        summary_ru: r.get(4)?,
                        topic: r.get(5)?,
                        lang_ru: r.get(6)?,
                        published_at: r.get(7)?,
                        read: r.get(8)?,
                        comments_url: r.get(9)?,
                    })
                },
            )
            .optional()
        })
        .await
}

/// Кэш полного RU-текста статьи (NF-6): абзацы одной строкой через пустую строку + флаг усечения.
pub async fn get_body(reader: &ReadPool, id: i64) -> DbResult<Option<(String, bool)>> {
    reader
        .query(move |c| {
            c.query_row(
                "SELECT body_ru, body_truncated FROM news_items WHERE id=?1 AND body_ru IS NOT NULL",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
            )
            .optional()
        })
        .await
}

/// Пишет кэш тела статьи (NF-6); повторный фетч просто перезаписывает.
pub async fn set_body(
    writer: &WriteActor,
    id: i64,
    body: String,
    truncated: bool,
    now: i64,
) -> DbResult<()> {
    writer
        .call(move |c| {
            c.execute(
                "UPDATE news_items SET body_ru=?1, body_truncated=?2, body_fetched_at=?3 WHERE id=?4",
                params![body, truncated, now, id],
            )?;
            Ok(())
        })
        .await
}

/// Ретенция (AC-NF-5): удаляет items и runs старше [`RETENTION_DAYS`] от `now`. Возвращает
/// число удалённых записей ленты.
pub async fn retention_gc(writer: &WriteActor, now: i64) -> DbResult<usize> {
    writer
        .call(move |c| {
            let cutoff = now - RETENTION_DAYS * 86_400;
            let n = c.execute("DELETE FROM news_items WHERE fetched_at < ?1", [cutoff])?;
            c.execute("DELETE FROM news_runs WHERE run_at < ?1", [cutoff])?;
            Ok(n)
        })
        .await
}

/// Фиксирует итог прогона (сводка + статы + видимые ошибки источников + LLM-down-сигнал B12).
pub async fn record_run(writer: &WriteActor, run: NewsRun) -> DbResult<()> {
    writer
        .call(move |c| {
            let errors = serde_json::to_string(&run.errors).unwrap_or_else(|_| "[]".into());
            // B12: NULL — сбоя вызова LLM не было; JSON {endpoint, partial} — был.
            let llm_down = run
                .llm_down
                .as_ref()
                .and_then(|i| serde_json::to_string(i).ok());
            c.execute(
                "INSERT INTO news_runs(run_at,digest_ru,items_new,sources_ok,sources_total,\
                 llm_failed,errors,llm_down) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    run.run_at,
                    run.digest_ru,
                    run.items_new,
                    run.sources_ok,
                    run.sources_total,
                    run.llm_failed,
                    errors,
                    llm_down
                ],
            )?;
            Ok(())
        })
        .await
}

/// История прогонов (W-39 «Диагностика»): свежие сверху, `limit` записей. Без безлимитных выгрузок
/// (урок #22) — вызывающий передаёт разумный кэп. Пусто — прогонов ещё не было.
pub async fn list_runs(reader: &ReadPool, limit: i64) -> DbResult<Vec<NewsRun>> {
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT run_at,digest_ru,items_new,sources_ok,sources_total,llm_failed,errors,\
                 llm_down FROM news_runs ORDER BY run_at DESC, id DESC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([limit], |r| {
                    let errors_raw: String = r.get(6)?;
                    let llm_down_raw: Option<String> = r.get(7)?;
                    Ok(NewsRun {
                        run_at: r.get(0)?,
                        digest_ru: r.get(1)?,
                        items_new: r.get(2)?,
                        sources_ok: r.get(3)?,
                        sources_total: r.get(4)?,
                        llm_failed: r.get(5)?,
                        errors: serde_json::from_str(&errors_raw).unwrap_or_default(),
                        llm_down: llm_down_raw.and_then(|s| serde_json::from_str(&s).ok()),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Последний прогон (шапка страницы); `None` — прогонов ещё не было.
pub async fn latest_run(reader: &ReadPool) -> DbResult<Option<NewsRun>> {
    reader
        .query(|c| {
            c.query_row(
                "SELECT run_at,digest_ru,items_new,sources_ok,sources_total,llm_failed,errors,\
                 llm_down FROM news_runs ORDER BY run_at DESC, id DESC LIMIT 1",
                [],
                |r| {
                    let errors_raw: String = r.get(6)?;
                    let llm_down_raw: Option<String> = r.get(7)?;
                    Ok(NewsRun {
                        run_at: r.get(0)?,
                        digest_ru: r.get(1)?,
                        items_new: r.get(2)?,
                        sources_ok: r.get(3)?,
                        sources_total: r.get(4)?,
                        llm_failed: r.get(5)?,
                        errors: serde_json::from_str(&errors_raw).unwrap_or_default(),
                        llm_down: llm_down_raw.and_then(|s| serde_json::from_str(&s).ok()),
                    })
                },
            )
            .optional()
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::TempDir;

    async fn open() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("nexus.db")).await.unwrap();
        (dir, db)
    }

    fn row(url: &str, topic: &str) -> NewRow {
        NewRow {
            source_id: "openai".into(),
            url: url.into(),
            title: "Original".into(),
            title_ru: "Заголовок".into(),
            summary_ru: "Резюме.".into(),
            topic: topic.into(),
            lang_ru: false,
            published_at: 1_750_000_000,
            comments_url: None,
        }
    }

    /// AC-NF-4: повторный прогон тех же url не плодит дублей; обновлённый title по тому же url
    /// НЕ перетирает прочитанность (ON CONFLICT DO NOTHING).
    #[tokio::test]
    async fn dedup_by_url_preserves_read_state() {
        let (_d, db) = open().await;
        let (w, r) = (db.writer(), db.reader());

        assert_eq!(
            insert_items(w, vec![row("https://a/1", "Модели")], 100)
                .await
                .unwrap(),
            1
        );
        let id = list_items(r, None, false, 10, 0).await.unwrap()[0].id;
        mark_read(w, id, true, 150).await.unwrap();

        // Тот же url с «обновлённым» заголовком + один новый.
        let mut updated = row("https://a/1", "Модели");
        updated.title_ru = "Новый заголовок".into();
        let n = insert_items(w, vec![updated, row("https://a/2", "Модели")], 200)
            .await
            .unwrap();
        assert_eq!(n, 1, "вставлен только новый url");

        let items = list_items(r, None, false, 10, 0).await.unwrap();
        assert_eq!(items.len(), 2);
        let first = items.iter().find(|i| i.id == id).unwrap();
        assert!(first.read, "прочитанность пережила повторный прогон");
        assert_eq!(
            first.title_ru, "Заголовок",
            "title не перетёрт (DO NOTHING)"
        );
    }

    /// NF-6: кэш тела статьи — roundtrip с флагом усечения (миграция 011); без кэша — `None`.
    #[tokio::test]
    async fn body_cache_roundtrip() {
        let (_d, db) = open().await;
        let (w, r) = (db.writer(), db.reader());
        insert_items(w, vec![row("https://a/1", "Модели")], 100)
            .await
            .unwrap();
        let id = list_items(r, None, false, 10, 0).await.unwrap()[0].id;

        assert!(get_body(r, id).await.unwrap().is_none(), "кэша ещё нет");
        set_body(w, id, "Абзац один.\n\nАбзац два.".into(), true, 200)
            .await
            .unwrap();
        let (body, truncated) = get_body(r, id).await.unwrap().expect("кэш записан");
        assert_eq!(body.split("\n\n").count(), 2);
        assert!(truncated, "флаг усечения сохранён");
    }

    /// Фильтры ленты: тема/непрочитанные/скрытые/пагинация; темы — по убыванию частоты.
    #[tokio::test]
    async fn list_filters_topics_and_pagination() {
        let (_d, db) = open().await;
        let (w, r) = (db.writer(), db.reader());
        insert_items(
            w,
            vec![
                row("https://a/1", "Модели"),
                row("https://a/2", "Модели"),
                row("https://a/3", "Инференс"),
            ],
            100,
        )
        .await
        .unwrap();
        let id1 = list_items(r, None, false, 10, 0).await.unwrap()[0].id;
        mark_read(w, id1, true, 150).await.unwrap();

        assert_eq!(
            list_items(r, Some("Модели".into()), false, 10, 0)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            list_items(r, None, true, 10, 0).await.unwrap().len(),
            2,
            "unread_only"
        );
        assert_eq!(
            list_items(r, None, false, 2, 0).await.unwrap().len(),
            2,
            "limit"
        );
        assert_eq!(
            list_items(r, None, false, 2, 2).await.unwrap().len(),
            1,
            "offset"
        );
        assert_eq!(
            list_topics(r).await.unwrap(),
            vec!["Модели".to_string(), "Инференс".to_string()]
        );
    }

    /// AC-NF-5: ретенция удаляет старые items и runs, свежие живут; read-флаг живёт до ретенции.
    #[tokio::test]
    async fn retention_removes_only_old() {
        let (_d, db) = open().await;
        let (w, r) = (db.writer(), db.reader());
        let day = 86_400;
        let now = 100 * day;
        insert_items(
            w,
            vec![row("https://old/1", "Т")],
            now - (RETENTION_DAYS + 1) * day,
        )
        .await
        .unwrap();
        insert_items(w, vec![row("https://new/1", "Т")], now - day)
            .await
            .unwrap();
        record_run(
            w,
            NewsRun {
                run_at: now - (RETENTION_DAYS + 1) * day,
                digest_ru: "старая".into(),
                items_new: 1,
                sources_ok: 1,
                sources_total: 1,
                llm_failed: 0,
                errors: vec![],
                llm_down: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            retention_gc(w, now).await.unwrap(),
            1,
            "удалена только старая запись"
        );
        let left = list_items(r, None, false, 10, 0).await.unwrap();
        assert_eq!(left.len(), 1);
        assert!(left[0].url.contains("new"));
        assert!(latest_run(r).await.unwrap().is_none(), "старый run вычищен");
    }

    /// Сводка прогона: запись/чтение последней, ошибки источников — JSON-roundtrip (no silent caps).
    /// B12: llm_down здесь None → NULL-колонка → None при чтении (здоровый прогон).
    #[tokio::test]
    async fn run_record_and_latest_roundtrip() {
        let (_d, db) = open().await;
        let (w, r) = (db.writer(), db.reader());
        record_run(
            w,
            NewsRun {
                run_at: 100,
                digest_ru: "Сводка.".into(),
                items_new: 7,
                sources_ok: 15,
                sources_total: 16,
                llm_failed: 2,
                errors: vec!["mistral: timeout".into()],
                llm_down: None,
            },
        )
        .await
        .unwrap();
        let run = latest_run(r).await.unwrap().expect("прогон записан");
        assert_eq!(run.digest_ru, "Сводка.");
        assert_eq!(
            (
                run.items_new,
                run.sources_ok,
                run.sources_total,
                run.llm_failed
            ),
            (7, 15, 16, 2)
        );
        assert_eq!(run.errors, vec!["mistral: timeout".to_string()]);
        assert_eq!(run.llm_down, None, "здоровый прогон → llm_down NULL");
    }

    /// B12: структурный LLM-down-сигнал переживает запись/чтение (latest_run и list_runs) —
    /// endpoint и partial возвращаются как записаны, без строкового сниффинга.
    #[tokio::test]
    async fn run_llm_down_roundtrip() {
        let (_d, db) = open().await;
        let (w, r) = (db.writer(), db.reader());
        let info = LlmDownInfo {
            endpoint: Some("http://192.168.0.31:8084".into()),
            partial: false,
        };
        record_run(
            w,
            NewsRun {
                run_at: 100,
                digest_ru: String::new(),
                items_new: 0,
                sources_ok: 1,
                sources_total: 1,
                llm_failed: 4,
                errors: vec!["Анализатор новостей недоступен: …".into()],
                llm_down: Some(info.clone()),
            },
        )
        .await
        .unwrap();
        // Второй прогон — частичный сбой без названного эндпоинта.
        record_run(
            w,
            NewsRun {
                run_at: 200,
                digest_ru: "Сводка.".into(),
                items_new: 3,
                sources_ok: 1,
                sources_total: 1,
                llm_failed: 1,
                errors: vec![],
                llm_down: Some(LlmDownInfo {
                    endpoint: None,
                    partial: true,
                }),
            },
        )
        .await
        .unwrap();

        let latest = latest_run(r).await.unwrap().expect("прогон записан");
        assert_eq!(
            latest.llm_down,
            Some(LlmDownInfo {
                endpoint: None,
                partial: true
            })
        );
        let all = list_runs(r, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].llm_down, Some(info), "endpoint+partial как записаны");
    }

    /// W-39: история прогонов отдаётся в порядке убывания run_at и уважает limit; ошибки —
    /// JSON-roundtrip как в `latest_run`. Пустая таблица → пустой вектор.
    #[tokio::test]
    async fn list_runs_orders_desc_and_limits() {
        let (_d, db) = open().await;
        let (w, r) = (db.writer(), db.reader());

        assert!(
            list_runs(r, 10).await.unwrap().is_empty(),
            "пусто без прогонов"
        );

        for (run_at, items_new) in [(100, 1), (300, 3), (200, 2)] {
            record_run(
                w,
                NewsRun {
                    run_at,
                    digest_ru: format!("прогон {run_at}"),
                    items_new,
                    sources_ok: 1,
                    sources_total: 1,
                    llm_failed: 0,
                    errors: if run_at == 300 {
                        vec!["mistral: timeout".into()]
                    } else {
                        vec![]
                    },
                    llm_down: None,
                },
            )
            .await
            .unwrap();
        }

        // Свежие сверху (по run_at DESC).
        let all = list_runs(r, 10).await.unwrap();
        assert_eq!(
            all.iter().map(|x| x.run_at).collect::<Vec<_>>(),
            vec![300, 200, 100]
        );
        assert_eq!(all[0].errors, vec!["mistral: timeout".to_string()]);

        // Лимит режет хвост, сохраняя порядок.
        let two = list_runs(r, 2).await.unwrap();
        assert_eq!(two.len(), 2);
        assert_eq!(
            two.iter().map(|x| x.run_at).collect::<Vec<_>>(),
            vec![300, 200]
        );
    }
}
