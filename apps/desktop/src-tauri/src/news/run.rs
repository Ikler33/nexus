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

/// W-2: строка об недоступности LLM-оценки для `errors[]`, двухуровневая (чтобы НЕ врать «лента не
/// обновится» при частичном/транзиентном сбое одного батча из многих, ревью W-2):
/// - `items_new == 0` → эндпоинт фактически недоступен весь прогон (тотально, лента пуста);
/// - `items_new > 0`  → часть батчей прошла, лента обновлена частично (мягкая формулировка).
///
/// Тотальная строка начинается с «Анализатор новостей недоступен» — по этому префиксу фронт
/// поднимает верхний баннер (частичную — только в раскрываемом списке ошибок прогона, без тревоги).
fn llm_unavailable_msg(endpoint: Option<&str>, llm_failed: i64, items_new: i64) -> String {
    let ep = endpoint
        .map(|u| u.trim())
        .filter(|u| !u.is_empty())
        .unwrap_or("(эндпоинт ИИ не задан)");
    if items_new == 0 {
        format!(
            "Анализатор новостей недоступен: {ep} — {llm_failed} зап. не оценены; лента не обновится, \
             пока эндпоинт не починить (Настройки → ИИ)"
        )
    } else {
        format!(
            "ИИ-анализатор частично недоступен: {ep} — {llm_failed} зап. не оценены в этот прогон \
             (повтор при следующем обновлении); остальные новости добавлены"
        )
    }
}

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
    // W-2: URL LLM-эндпоинта оценки (для видимой ошибки при недоступности). `None` → не назван.
    chat_endpoint: Option<&str>,
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
    // W-2: батчи, чей ВЫЗОВ упал (эндпоинт недоступен) — отличаем от парс-фейлов отдельных записей,
    // чтобы баннер «анализатор недоступен» не загорался на паре кривых JSON при живом эндпоинте.
    let mut batch_errors = 0usize;
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
        batch_errors += report.batch_errors;
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

    // W-2: сбой ВЫЗОВА LLM-оценки (мёртвый/недоступный эндпоинт — дрейф .31 вместо .28) раньше был
    // лишь счётчиком `llm_failed`, невидимым в errors[]/dead-jobs → новости «не грузились» без причины.
    // Делаем ВИДИМЫМ одной строкой в errors[], называющей эндпоинт. Двухуровнево (после insert_items,
    // когда известно items_new), чтобы НЕ врать «лента не обновится» при ЧАСТИЧНОМ/транзиентном сбое
    // одного батча из многих (ревью W-2): items_new==0 → эндпоинт фактически недоступен весь прогон;
    // items_new>0 → часть прошла, лента обновлена частично. Неоценённые url не пишутся → повтор на
    // следующем прогоне, когда эндпоинт оживёт (graceful degrade без dedup-«вечно неоценено»).
    // B12: тот же сигнал — ещё и структурным полем (фронт ключуется на него, не на RU-префикс);
    // человекочитаемая строка в errors[] остаётся (видимый список ошибок прогона).
    let llm_down = (batch_errors > 0).then(|| super::LlmDownInfo {
        endpoint: chat_endpoint
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string),
        partial: items_new > 0,
    });
    if batch_errors > 0 {
        errors.push(llm_unavailable_msg(chat_endpoint, llm_failed, items_new));
    }

    let run = NewsRun {
        run_at: now,
        digest_ru,
        items_new,
        sources_ok,
        sources_total: sources.len() as i64,
        llm_failed,
        errors,
        llm_down,
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
    /// W-40: ОБА слота модели — выбор на КАЖДЫЙ прогон по `news.json::model_pref` (горячее
    /// переключение, как `enabled`/keywords). `chat_util` = ai.fast (дефолт/`"fast"`),
    /// `chat_fast` = ai.chat (`"main"`). Зеркалит `select_news_chat` (on-demand ридер) — теперь
    /// плановый сбор/анализ/дайджест честно идёт выбранной моделью, а не жёстко fast.
    pub chat_util: Option<Arc<dyn ChatProvider>>,
    pub chat_fast: Option<Arc<dyn ChatProvider>>,
    pub writer: WriteActor,
    pub reader: ReadPool,
    /// Путь `news.json` (OS config-dir; резолвится в open_vault — у хендлера нет AppHandle).
    pub config_path: std::path::PathBuf,
    /// Сток этапного прогресса (`news:progress` для UI); тестам — no-op.
    pub progress: Arc<NewsProgress>,
    /// W-2/W-40: URL утилитарной (ai.fast) и основной (ai.chat) моделей — для видимой ошибки по
    /// ВЫБРАННОЙ модели при недоступности.
    pub url_util: Option<String>,
    pub url_fast: Option<String>,
}

/// W-40: выбор слота модели новостей по `model_pref` — `"main"` → основной слот (ai.chat) с
/// фолбэком на утилитарный; иначе (`"fast"`/`None`/прочее) → утилитарный (ai.fast). Параметризован
/// по `T`, чтобы провайдер и его URL выбирались СОГЛАСОВАННО. Зеркалит `select_news_chat`
/// (on-demand ридер) — плановый прогон теперь честно идёт ВЫБРАННОЙ моделью, а не жёстко fast.
fn pick_news_by_pref<T: Clone>(
    util: &Option<T>,
    fast: &Option<T>,
    pref: Option<&str>,
) -> Option<T> {
    if matches!(pref, Some("main")) {
        fast.clone().or_else(|| util.clone())
    } else {
        util.clone().or_else(|| fast.clone())
    }
}

#[async_trait]
impl JobHandler for NewsFeedHandler {
    async fn handle(&self, _job: &Job) -> Result<(), String> {
        let cfg = super::load_news_config(&self.config_path);
        if !cfg.enabled {
            tracing::debug!("news: фича выключена — прогон пропущен (consent, AC-NF-7)");
            return Ok(());
        }
        // W-40: модель прогона по `model_pref` (читается на КАЖДЫЙ прогон → горячее переключение).
        // Провайдер и его URL выбираются СОГЛАСОВАННО одним правилом (`pick_news_by_pref`).
        let pref = cfg.model_pref.as_deref();
        let chat_endpoint = pick_news_by_pref(&self.url_util, &self.url_fast, pref);
        let Some(chat) = pick_news_by_pref(&self.chat_util, &self.chat_fast, pref) else {
            tracing::warn!("news: нет настроенного chat-провайдера — прогон пропущен");
            return Ok(());
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let run = run_news_pipeline(
            &*self.fetcher,
            &chat,
            &self.writer,
            &self.reader,
            &cfg,
            crate::scheduler::now_secs(),
            &cancel,
            &*self.progress,
            chat_endpoint.as_deref(),
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

    /// Мок-LLM с мёртвым эндпоинтом: любой вызов оценки → ошибка (имитация недоступного .31/.28).
    struct DeadChat;
    #[async_trait]
    impl ChatProvider for DeadChat {
        async fn stream_chat(
            &self,
            _messages: &[ChatMessage],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
        ) -> AiResult<String> {
            Err(crate::ai::AiError::Http("connection refused".into()))
        }
        fn model_id(&self) -> &str {
            "dead"
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
            None,
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
        assert_eq!(
            run.llm_down, None,
            "LLM жив → структурного сигнала нет (B12)"
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
            None,
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

    /// W-2: мёртвый LLM-эндпоинт = ВИДИМЫЙ сбой, а не молчаливо пустая лента. Прогон остаётся `Ok`
    /// (DB-save/record_run проходят), но: (a) `llm_failed` = числу записей; (b) errors[] содержит
    /// ОДНУ строку, называющую эндпоинт; (c) записи НЕ сохранены (повтор оживёт на починке).
    #[tokio::test]
    async fn dead_llm_endpoint_is_visible_not_silent() {
        let (_d, db) = open().await;
        let fetcher = MockFetcher {
            bodies: HashMap::from([(
                "https://openai.com/news/rss.xml",
                include_str!("fixtures/openai_rss.xml"),
            )]),
            calls: AtomicUsize::new(0),
        };
        let chat: Arc<dyn ChatProvider> = Arc::new(DeadChat);
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
            Some("http://192.168.0.31:8084"),
        )
        .await
        .expect("прогон не падает при мёртвом LLM (Ok-путь, DB save/record_run проходят)");

        // (a) фид всё равно опрошен; (b) ничего не сохранено (нечего показать без оценки); (c) счётчик.
        assert_eq!(
            run.sources_ok, 1,
            "фид openai опрошен несмотря на мёртвый LLM"
        );
        assert_eq!(
            run.items_new, 0,
            "неоценённые записи НЕ сохранены (повтор оживёт)"
        );
        assert!(
            run.llm_failed >= 4,
            "все записи фикстуры посчитаны как failed"
        );

        // Главное: сбой ВИДИМ и НАЗЫВАЕТ эндпоинт — ровно одна такая строка (без спама по батчам).
        let named: Vec<&String> = run
            .errors
            .iter()
            .filter(|e| e.contains("Анализатор новостей недоступен"))
            .collect();
        assert_eq!(named.len(), 1, "ровно одна ошибка-аналайзер (не по батчам)");
        assert!(
            named[0].contains("192.168.0.31:8084"),
            "ошибка называет недостижимый эндпоинт: {}",
            named[0]
        );

        // B12: тот же сигнал — структурным полем (фронт ключуется на него, не на RU-префикс).
        assert_eq!(
            run.llm_down,
            Some(super::super::LlmDownInfo {
                endpoint: Some("http://192.168.0.31:8084".into()),
                partial: false, // items_new == 0 → тотальный сбой (баннер)
            })
        );

        // Лента действительно пуста (UI покажет баннер + причину, не молчаливый «нет новостей»).
        let items = super::super::list_items(db.reader(), None, false, 50, 0)
            .await
            .unwrap();
        assert!(items.is_empty());
    }

    /// W-2 (ревью): двухуровневая формулировка — НЕ врём «лента не обновится» при частичном сбое.
    #[test]
    fn llm_unavailable_msg_is_two_tier() {
        // Тотально (ничего не сохранено) — префикс «Анализатор новостей недоступен» (фронт → баннер).
        let total = llm_unavailable_msg(Some("http://192.168.0.31:8084"), 12, 0);
        assert!(total.starts_with("Анализатор новостей недоступен"));
        assert!(total.contains("192.168.0.31:8084"));
        assert!(total.contains("лента не обновится"));
        // Частично (часть сохранена) — мягкая формулировка, НЕ обещает, что лента не обновилась.
        let partial = llm_unavailable_msg(Some("http://h:8084"), 3, 110);
        assert!(partial.starts_with("ИИ-анализатор частично недоступен"));
        assert!(!partial.contains("лента не обновится"));
        assert!(partial.contains("остальные новости добавлены"));
        // Эндпоинт не задан — плейсхолдер вместо пустоты.
        assert!(llm_unavailable_msg(None, 1, 0).contains("эндпоинт ИИ не задан"));
        assert!(llm_unavailable_msg(Some("   "), 1, 0).contains("эндпоинт ИИ не задан"));
    }

    /// W-40: выбор слота модели по `model_pref` — `"main"`→основной (ai.chat), иначе→утилитарный
    /// (ai.fast, дефолт = прежнее поведение); фолбэки при отсутствии слота; оба None → None.
    #[test]
    fn pick_news_by_pref_routes_by_model_pref() {
        let util: Option<&str> = Some("util");
        let fast: Option<&str> = Some("fast");
        let none: Option<&str> = None;
        assert_eq!(pick_news_by_pref(&util, &fast, None), Some("util"));
        assert_eq!(pick_news_by_pref(&util, &fast, Some("fast")), Some("util"));
        assert_eq!(pick_news_by_pref(&util, &fast, Some("bogus")), Some("util"));
        assert_eq!(pick_news_by_pref(&util, &fast, Some("main")), Some("fast"));
        assert_eq!(pick_news_by_pref(&none, &fast, Some("fast")), Some("fast"));
        assert_eq!(pick_news_by_pref(&util, &none, Some("main")), Some("util"));
        assert_eq!(pick_news_by_pref(&none, &none, Some("main")), None);
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
            chat_util: Some(Arc::new(YesChat {
                eval_calls: AtomicUsize::new(0),
            })),
            chat_fast: None,
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            config_path,
            progress: Arc::new(|_, _, _| {}),
            url_util: None,
            url_fast: None,
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
            None,
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
            None,
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
