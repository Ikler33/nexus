//! Прогон ленты (NF-3, AC-NF-6): fetch → parse → keyword (этап 1) → LLM (этап 2) → store →
//! сводка дня → ретенция. Сетевой фетчер — трейт [`FeedFetcher`]: реальная реализация на
//! `GuardedClient`+`EgressFeature::NewsFeed` с лимитами W3 и DNS-гардом приходит срезом NF-4;
//! тесты гоняют пайплайн на мок-фетчере с фикстурами NF-1 и мок-LLM.
//!
//! Бюджеты — видимые (no silent caps): ошибки источников и LLM-отказы попадают в `news_runs`,
//! отрезанный по [`LLM_RUN_CAP`] излишек — строкой в errors.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;

use super::{
    daily_digest, evaluate_entries, keyword_filter, parse_feed, FeedKind, NewRow, NewsConfig,
    NewsEntry, NewsRun, Source,
};
use crate::ai::ChatProvider;
use crate::db::{DbResult, ReadPool, WriteActor};
use crate::scheduler::{Job, JobHandler};

/// Kind планировщика (run-if-overdue раз/сутки + manual «Обновить», AC-NF-6).
pub const KIND_NEWSFEED: &str = "newsfeed";
/// Потолок записей в LLM-этап за прогон (бюджет W3); излишек отрезается по дате с пометкой.
pub const LLM_RUN_CAP: usize = 60;
/// HN: запросов по ключам за прогон и записей на запрос (бюджет источника-агрегатора).
const HN_MAX_QUERIES: usize = 6;
const HN_HITS_PER_QUERY: usize = 10;

/// Доставка тела фида. `Err` — видимая ошибка ИСТОЧНИКА (прогон продолжается, AC-NF-1).
#[async_trait]
pub trait FeedFetcher: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<String, String>;
}

/// Колбэк этапного прогресса прогона (фидбэк владельца 11.06: «что сейчас происходит с лентой»):
/// `(этап, готово, всего)`; этапы: `sources` → `llm` → `digest` → `save`. Хендлер шлёт их
/// tauri-событием `news:progress`, UI показывает живой статус вместо немого «Собираю…».
pub type NewsProgress = dyn Fn(&str, usize, usize) + Send + Sync;

/// Полный прогон ленты. Вызывающий гарантирует `cfg.enabled` (хендлер гейтит до вызова).
/// 8 аргументов: пайплайн собирает независимые зависимости (фетчер/LLM/БД/конфиг/время/отмена/
/// прогресс) — группировка в структуру дала бы один одноразовый тип без выигрыша в ясности.
#[allow(clippy::too_many_arguments)]
pub async fn run_news_pipeline(
    fetcher: &dyn FeedFetcher,
    chat: &Arc<dyn ChatProvider>,
    writer: &WriteActor,
    reader: &ReadPool,
    cfg: &NewsConfig,
    now: i64,
    cancel: &Arc<AtomicBool>,
    progress: &NewsProgress,
) -> DbResult<NewsRun> {
    let sources = cfg.active_sources();
    let keywords = cfg.effective_keywords();
    let mut errors: Vec<String> = Vec::new();
    let mut sources_ok = 0i64;
    // (запись, lang_ru источника) — язык нужен LLM-этапу (D1: RU не «переводим»).
    let mut entries: Vec<(NewsEntry, bool)> = Vec::new();

    let total_sources = sources.len();
    for (i, s) in sources.iter().enumerate() {
        progress("sources", i, total_sources);
        match fetch_source(fetcher, s, &keywords).await {
            Ok(parsed) => {
                sources_ok += 1;
                // HN (Algolia) уже отфильтрован по ключам при fetch_source (по одному query на ключ).
                // Повторный keyword_filter по title+excerpt — двойная фильтрация, к тому же ВРЕДНАЯ:
                // `parse_hn` кладёт story_text=null → excerpt="", поэтому запись, где ключ был только
                // в теле (его нашёл Algolia), здесь отбрасывалась бы как «нет ключа» (находка аудита B14).
                let kept = if matches!(s.kind, FeedKind::HnAlgolia) {
                    parsed
                } else {
                    keyword_filter(parsed, s, &keywords)
                };
                entries.extend(kept.into_iter().map(|e| (e, s.lang_ru)));
            }
            Err(e) => errors.push(format!("{}: {e}", s.id)),
        }
    }

    // Префильтр против БД: уже виденные url не жгут LLM (дедуп хранения — отдельно, AC-NF-4).
    let urls: Vec<String> = entries.iter().map(|(e, _)| e.url.clone()).collect();
    let fresh_urls = super::filter_new_urls(reader, urls).await?;
    entries.retain(|(e, _)| fresh_urls.contains(&e.url));

    // Бюджет LLM-этапа: свежие сначала, излишек отрезается ВИДИМО (no silent caps).
    entries.sort_by_key(|(e, _)| std::cmp::Reverse(e.published_at));
    if entries.len() > LLM_RUN_CAP {
        errors.push(format!(
            "llm-бюджет: обработано {LLM_RUN_CAP} из {} свежих записей (старшие отрезаны)",
            entries.len()
        ));
        entries.truncate(LLM_RUN_CAP);
    }

    // LLM-этап двумя языковыми группами (инструкция различается, D1).
    let total_entries = entries.len();
    let llm_done = std::sync::atomic::AtomicUsize::new(0);
    progress("llm", 0, total_entries);
    let mut llm_failed = 0i64;
    let mut rows: Vec<NewRow> = Vec::new();
    for lang_ru in [false, true] {
        let group: Vec<NewsEntry> = entries
            .iter()
            .filter(|(_, ru)| *ru == lang_ru)
            .map(|(e, _)| e.clone())
            .collect();
        if group.is_empty() {
            continue;
        }
        let report = evaluate_entries(chat, &group, lang_ru, cancel, &|batch_done| {
            let done =
                llm_done.fetch_add(batch_done, std::sync::atomic::Ordering::Relaxed) + batch_done;
            progress("llm", done.min(total_entries), total_entries);
        })
        .await;
        llm_failed += report.failed as i64;
        rows.extend(report.items.into_iter().map(|ev| NewRow {
            source_id: ev.entry.source_id.clone(),
            url: ev.entry.url.clone(),
            title: ev.entry.title.clone(),
            title_ru: ev.title_ru,
            summary_ru: ev.summary_ru,
            topic: ev.topic,
            lang_ru,
            published_at: ev.entry.published_at,
            comments_url: ev.entry.comments_url.clone(),
        }));
    }

    progress("digest", 0, 1);
    // Сводка дня — по тому, что реально нового (пусто → '' , UI покажет «нет новостей»).
    let evaluated_for_digest: Vec<super::EvaluatedEntry> = rows
        .iter()
        .map(|r| super::EvaluatedEntry {
            entry: NewsEntry {
                source_id: r.source_id.clone(),
                url: r.url.clone(),
                title: r.title.clone(),
                published_at: r.published_at,
                excerpt: String::new(),
                comments_url: r.comments_url.clone(),
            },
            title_ru: r.title_ru.clone(),
            summary_ru: r.summary_ru.clone(),
            topic: r.topic.clone(),
        })
        .collect();
    let digest_ru = if evaluated_for_digest.is_empty() {
        String::new()
    } else {
        daily_digest(chat, &evaluated_for_digest, cancel)
            .await
            .unwrap_or_else(|e| {
                errors.push(format!("сводка: {e}"));
                String::new()
            })
    };

    let items_new = super::insert_items(writer, rows, now).await? as i64;
    super::retention_gc(writer, now).await?;

    let run = NewsRun {
        run_at: now,
        digest_ru,
        items_new,
        sources_ok,
        sources_total: sources.len() as i64,
        llm_failed,
        errors,
    };
    super::record_run(writer, run.clone()).await?;
    Ok(run)
}

/// Фетч одного источника. HN — несколько запросов по ключам (агрегатор, AC: ключи задают
/// охват); остальные — один URL из реестра.
async fn fetch_source(
    fetcher: &dyn FeedFetcher,
    s: &Source,
    keywords: &[String],
) -> Result<Vec<NewsEntry>, String> {
    if !matches!(s.kind, FeedKind::HnAlgolia) {
        let body = fetcher.fetch(s.url).await?;
        return parse_feed(s.kind, s.id, &body).map_err(|e| e.to_string());
    }
    // HN: по запросу на ключ (Algolia не умеет OR в query) — бюджет HN_MAX_QUERIES×HITS.
    let mut all = Vec::new();
    let mut last_err = None;
    let mut any_ok = false;
    for kw in keywords.iter().take(HN_MAX_QUERIES) {
        let url = format!(
            "{}&hitsPerPage={HN_HITS_PER_QUERY}",
            s.url.replace("{query}", &percent_encode(kw))
        );
        match fetcher.fetch(&url).await {
            Ok(body) => match parse_feed(s.kind, s.id, &body) {
                Ok(es) => {
                    any_ok = true;
                    all.extend(es);
                }
                Err(e) => last_err = Some(e.to_string()),
            },
            Err(e) => last_err = Some(e),
        }
    }
    if !any_ok {
        return Err(last_err.unwrap_or_else(|| "нет ключевых слов для запросов".into()));
    }
    // Дедуп между ключами (одна история матчит несколько ключей).
    let mut seen = std::collections::HashSet::new();
    all.retain(|e| seen.insert(e.url.clone()));
    Ok(all)
}

/// Минимальный percent-encode значения query-параметра (ascii-буквы/цифры/`-_.~` как есть).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Хендлер планировщика (AC-NF-6): перечитывает конфиг на каждый прогон (тоггл со страницы),
/// выключенная фича → штатный no-op `Ok` (НЕ failed — это consent-состояние, не сбой).
pub struct NewsFeedHandler {
    pub fetcher: Arc<dyn FeedFetcher>,
    pub chat: Arc<dyn ChatProvider>,
    pub writer: WriteActor,
    pub reader: ReadPool,
    /// Путь `news.json` (OS config-dir; резолвится в open_vault — у хендлера нет AppHandle).
    pub config_path: std::path::PathBuf,
    /// Сток этапного прогресса (`news:progress` для UI); тестам — no-op.
    pub progress: Arc<NewsProgress>,
}

#[async_trait]
impl JobHandler for NewsFeedHandler {
    async fn handle(&self, _job: &Job) -> Result<(), String> {
        let cfg = super::load_news_config(&self.config_path);
        if !cfg.enabled {
            tracing::debug!("news: фича выключена — прогон пропущен (consent, AC-NF-7)");
            return Ok(());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let run = run_news_pipeline(
            &*self.fetcher,
            &self.chat,
            &self.writer,
            &self.reader,
            &cfg,
            crate::scheduler::now_secs(),
            &cancel,
            &*self.progress,
        )
        .await
        .map_err(|e| e.to_string())?;
        (self.progress)("save", 1, 1);
        tracing::info!(
            new = run.items_new,
            sources = format!("{}/{}", run.sources_ok, run.sources_total),
            llm_failed = run.llm_failed,
            "news: прогон завершён"
        );
        Ok(())
    }

    /// LLM-этап уступает интерактивному чату/inline (S5).
    fn defer_under_interactive(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{AiResult, ChatMessage};
    use crate::db::Database;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// Мок-фетчер: url → фикстура; неизвестный url → ошибка источника.
    struct MockFetcher {
        bodies: HashMap<&'static str, &'static str>,
        calls: AtomicUsize,
    }
    #[async_trait]
    impl FeedFetcher for MockFetcher {
        async fn fetch(&self, url: &str) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.bodies
                .get(url)
                .map(|b| b.to_string())
                .ok_or_else(|| "недоступен".to_string())
        }
    }

    /// Мок-LLM: relevant=true на всё, считает вызовы оценки.
    struct YesChat {
        eval_calls: AtomicUsize,
    }
    #[async_trait]
    impl ChatProvider for YesChat {
        async fn stream_chat(
            &self,
            messages: &[ChatMessage],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
        ) -> AiResult<String> {
            let user = &messages[1].content;
            if user.contains("Заголовок:") {
                self.eval_calls.fetch_add(1, Ordering::SeqCst);
                // Отвечаем на все индексы батча.
                let n = user.matches("Заголовок:").count();
                let arr: Vec<String> = (0..n)
                    .map(|i| {
                        format!(
                            "{{\"i\":{i},\"relevant\":true,\"title_ru\":\"З{i}\",\
                             \"summary_ru\":\"Р{i}.\",\"topic\":\"Тема\"}}"
                        )
                    })
                    .collect();
                Ok(format!("[{}]", arr.join(",")))
            } else {
                Ok("Сводка дня.".into())
            }
        }
        fn model_id(&self) -> &str {
            "mock"
        }
    }

    async fn open() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("nexus.db")).await.unwrap();
        (dir, db)
    }

    fn cfg_two_sources() -> NewsConfig {
        // Только openai (RSS-фикстура) + заведомо падающий deepmind; остальное выключаем.
        let mut cfg = NewsConfig {
            enabled: true,
            ..Default::default()
        };
        for s in super::super::SOURCES_V1 {
            cfg.sources.insert(s.id.to_string(), false);
        }
        cfg.sources.insert("openai".into(), true);
        cfg.sources.insert("deepmind".into(), true);
        cfg
    }

    /// Сквозной прогон (AC-NF-1/6): живой источник разобран и сохранён, упавший — видимой
    /// ошибкой; сводка записана; ПОВТОРНЫЙ прогон не плодит дублей и НЕ жжёт LLM (префильтр).
    #[tokio::test]
    async fn pipeline_end_to_end_with_visible_errors_and_no_rerun_llm() {
        let (_d, db) = open().await;
        let fetcher = MockFetcher {
            bodies: HashMap::from([(
                "https://openai.com/news/rss.xml",
                include_str!("fixtures/openai_rss.xml"),
            )]),
            calls: AtomicUsize::new(0),
        };
        let chat_impl = Arc::new(YesChat {
            eval_calls: AtomicUsize::new(0),
        });
        let chat: Arc<dyn ChatProvider> = chat_impl.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let cfg = cfg_two_sources();

        let run = run_news_pipeline(
            &fetcher,
            &chat,
            db.writer(),
            db.reader(),
            &cfg,
            1_800_000_000,
            &cancel,
            &|_, _, _| {},
        )
        .await
        .unwrap();
        assert_eq!(run.items_new, 4, "4 записи фикстуры openai сохранены");
        assert_eq!((run.sources_ok, run.sources_total), (1, 2));
        assert_eq!(
            run.errors,
            vec!["deepmind: недоступен".to_string()],
            "падение источника видимо"
        );
        assert_eq!(run.digest_ru, "Сводка дня.");
        assert_eq!(
            chat_impl.eval_calls.load(Ordering::SeqCst),
            1,
            "один батч оценки"
        );

        // Повторный прогон: дублей нет, LLM не вызывался повторно (префильтр по url).
        let run2 = run_news_pipeline(
            &fetcher,
            &chat,
            db.writer(),
            db.reader(),
            &cfg,
            1_800_000_100,
            &cancel,
            &|_, _, _| {},
        )
        .await
        .unwrap();
        assert_eq!(run2.items_new, 0, "дедуп между прогонами (AC-NF-4)");
        assert_eq!(
            chat_impl.eval_calls.load(Ordering::SeqCst),
            1,
            "LLM не жжётся на виденном"
        );
        assert_eq!(
            run2.digest_ru, "",
            "нечего сводить → пустая сводка (состояние UI)"
        );
        let items = super::super::list_items(db.reader(), None, false, 50, 0)
            .await
            .unwrap();
        assert_eq!(items.len(), 4);
    }

    /// AC-NF-6/7: выключенная фича → хендлер штатно no-op (Ok, не failed) и ничего не фетчит.
    #[tokio::test]
    async fn handler_noops_when_disabled() {
        let (_d, db) = open().await;
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("news.json"); // файла нет → дефолт enabled=false
        let fetcher = Arc::new(MockFetcher {
            bodies: HashMap::new(),
            calls: AtomicUsize::new(0),
        });
        let handler = NewsFeedHandler {
            fetcher: fetcher.clone(),
            chat: Arc::new(YesChat {
                eval_calls: AtomicUsize::new(0),
            }),
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            config_path,
            progress: Arc::new(|_, _, _| {}),
        };
        let job = Job {
            id: 1,
            kind: KIND_NEWSFEED.into(),
            payload: String::new(),
            state: "running".into(),
            run_at: 0,
            attempts: 0,
            max_attempts: 3,
            last_error: None,
        };
        handler.handle(&job).await.expect("no-op, не сбой");
        assert_eq!(
            fetcher.calls.load(Ordering::SeqCst),
            0,
            "сеть не тронута без consent"
        );
    }

    /// Бюджет LLM (cap 60): излишек отрезается с ВИДИМОЙ пометкой, обработаны самые свежие.
    #[tokio::test]
    async fn llm_cap_truncates_visibly_keeping_freshest() {
        let (_d, db) = open().await;
        // 70 синтетических записей в одном RSS (генерим тело фида).
        let items: String = (0..70)
            .map(|i| {
                format!(
                    "<item><title>T{i}</title><link>https://x/{i}</link>\
                     <pubDate>Tue, 10 Jun 2026 08:{:02}:00 GMT</pubDate>\
                     <description>D</description></item>",
                    i % 60
                )
            })
            .collect();
        let body: &'static str =
            Box::leak(format!("<rss><channel>{items}</channel></rss>").into_boxed_str());
        let fetcher = MockFetcher {
            bodies: HashMap::from([("https://openai.com/news/rss.xml", body)]),
            calls: AtomicUsize::new(0),
        };
        let chat: Arc<dyn ChatProvider> = Arc::new(YesChat {
            eval_calls: AtomicUsize::new(0),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let mut cfg = cfg_two_sources();
        cfg.sources.insert("deepmind".into(), false);

        let run = run_news_pipeline(
            &fetcher,
            &chat,
            db.writer(),
            db.reader(),
            &cfg,
            1_800_000_000,
            &cancel,
            &|_, _, _| {},
        )
        .await
        .unwrap();
        assert_eq!(run.items_new, LLM_RUN_CAP as i64, "обработан ровно бюджет");
        assert!(
            run.errors.iter().any(|e| e.contains("llm-бюджет")),
            "отрезание видимо (no silent caps): {:?}",
            run.errors
        );
    }

    /// HN-агрегатор: по запросу на ключ (≤6), дедуп между ключами, ключи percent-encoded.
    #[tokio::test]
    async fn hn_queries_per_keyword_with_dedup() {
        let hn = SOURCES_V1.iter().find(|s| s.id == "hn").unwrap();
        let hit = r#"{"hits":[{"title":"Post","url":"https://same/1","created_at_i":1765000000,"objectID":"1"}]}"#;
        let base = "https://hn.algolia.com/api/v1/search_by_date?tags=story&query=";
        let fetcher = MockFetcher {
            bodies: HashMap::from([
                (
                    Box::leak(format!("{base}llm&hitsPerPage=10").into_boxed_str()) as &'static str,
                    hit,
                ),
                (
                    Box::leak(format!("{base}c%2B%2B&hitsPerPage=10").into_boxed_str())
                        as &'static str,
                    hit,
                ),
            ]),
            calls: AtomicUsize::new(0),
        };
        let entries = fetch_source(&fetcher, hn, &["llm".into(), "c++".into()])
            .await
            .unwrap();
        assert_eq!(
            fetcher.calls.load(Ordering::SeqCst),
            2,
            "по запросу на ключ"
        );
        assert_eq!(entries.len(), 1, "дедуп одинаковых историй между ключами");
    }

    /// audit B14: HN (Algolia) НЕ прогоняется повторно через keyword_filter — иначе запись, где ключ
    /// был только в теле (story_text=null → excerpt=""), была бы отброшена по title+excerpt.
    #[tokio::test]
    async fn hn_keyword_filter_not_double_applied() {
        let (_d, db) = open().await;
        let base = "https://hn.algolia.com/api/v1/search_by_date?tags=story&query=";
        // Заголовок БЕЗ ключа "quantum" (ключ был в теле, его нашёл Algolia); excerpt пуст.
        let hit = r#"{"hits":[{"title":"Neat physics breakthrough","url":"https://hn/x","created_at_i":1765000000,"objectID":"1"}]}"#;
        let fetcher = MockFetcher {
            bodies: HashMap::from([(
                Box::leak(format!("{base}quantum&hitsPerPage=10").into_boxed_str()) as &'static str,
                hit,
            )]),
            calls: AtomicUsize::new(0),
        };
        let chat: Arc<dyn ChatProvider> = Arc::new(YesChat {
            eval_calls: AtomicUsize::new(0),
        });
        let cancel = Arc::new(AtomicBool::new(false));

        let mut cfg = NewsConfig {
            enabled: true,
            ..Default::default()
        };
        for s in super::super::SOURCES_V1 {
            cfg.sources.insert(s.id.to_string(), false);
        }
        cfg.sources.insert("hn".into(), true);
        cfg.keywords = Some(vec!["quantum".into()]);

        let run = run_news_pipeline(
            &fetcher,
            &chat,
            db.writer(),
            db.reader(),
            &cfg,
            1_800_000_000,
            &cancel,
            &|_, _, _| {},
        )
        .await
        .unwrap();
        assert_eq!(
            run.items_new, 1,
            "HN-запись с ключом в теле НЕ отброшена двойным фильтром (B14)"
        );
    }

    use super::super::SOURCES_V1;
}
