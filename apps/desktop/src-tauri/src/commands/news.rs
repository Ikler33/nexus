//! Команды ленты новостей (NF-3, AC-NF-9): страница читает ленту/темы/последний прогон,
//! отмечает прочитанное, сохраняет «в заметку» (AC-NF-11), дёргает ручной прогон и правит
//! конфиг (`news.json` в OS config-dir — consent-носитель, AC-NF-7). Сам прогон гоняет
//! планировщик (kind `newsfeed`); регистрация хендлера с реальным фетчером — срез NF-4.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::ai::{ChatProvider, LocalConfig};
use crate::db::{DbResult, ReadPool};
use crate::error::{AppError, AppResult};
use crate::net::{EgressFeature, GuardedClient, NetError, RunCtx};
use crate::news::{self, NewsConfig, NewsItem, NewsRun};
use crate::search::{self, SearchOptions};
use crate::state::AppState;
use crate::suggest::LinkSuggestion;

/// Размер страницы ленты (карточек за запрос) — без безлимитных выгрузок (урок #22).
const PAGE_SIZE: i64 = 50;

/// W-40: выбирает chat-провайдера пайплайна новостей по `model_pref` из `news.json`.
/// ЕДИНАЯ точка выбора (заменяет два инлайн-`.or_else`-места — `news_article`/`news_summarize`),
/// чтобы анализ статьи и её сокращение всегда шли через ОДНУ и ту же модель, что выбрал пользователь.
/// - `Some("main")` → основная модель `ai.chat` (= `chat_fast`) с fallback на утилитарную `ai.fast`.
/// - иначе (`Some("fast")` / `None` / неизвестное) → утилитарная `ai.fast` (= `chat_util`) с fallback
///   на `ai.chat` (= `chat_fast`). Это ТЕКУЩЕЕ (до-W-40) поведение — при `None` байт-в-байт прежнее.
fn select_news_chat(ai: &crate::ai::AIClient, pref: Option<&str>) -> Option<Arc<dyn ChatProvider>> {
    if pref == Some("main") {
        ai.chat_fast.clone().or_else(|| ai.chat_util.clone())
    } else {
        // fast / None / неизвестное — дефолт = прежнее поведение (ai.fast → ai.chat).
        ai.chat_util.clone().or_else(|| ai.chat_fast.clone())
    }
}

/// Страница ленты для UI: записи + чипы тем + шапка последнего прогона.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsPageDto {
    pub items: Vec<NewsItem>,
    pub topics: Vec<String>,
    pub run: Option<NewsRun>,
}

/// Лента (свежие сверху; скрытые не отдаются): фильтр по теме/непрочитанному, страница `page`.
#[tauri::command]
pub async fn get_news(
    state: State<'_, AppState>,
    topic: Option<String>,
    unread_only: Option<bool>,
    page: Option<u32>,
) -> AppResult<NewsPageDto> {
    let reader = state.vault().await?.db.reader().clone();
    let offset = i64::from(page.unwrap_or(0)) * PAGE_SIZE;
    let items = news::list_items(
        &reader,
        topic,
        unread_only.unwrap_or(false),
        PAGE_SIZE,
        offset,
    )
    .await?;
    let topics = news::list_topics(&reader).await?;
    let run = news::latest_run(&reader).await?;
    Ok(NewsPageDto { items, topics, run })
}

/// Потолок числа прогонов в истории «Диагностики» (W-39): без безлимитных выгрузок (урок #22) —
/// история компактная (компактный список последних прогонов), не отчёт.
const NEWS_RUNS_CAP: u32 = 50;
/// Таймаут пинга эндпоинта новостей (W-39): короткий — не вешать кнопку «Проверить связь».
const NEWS_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// W-39: история прогонов ленты для панели «Диагностика» — последние `limit` записей (свежие сверху).
/// Зеркалит `news::NewsRun` (camelCase): backend читает таблицу `news_runs` напрямую (read-many).
#[tauri::command]
pub async fn get_news_runs(state: State<'_, AppState>, limit: u32) -> AppResult<Vec<NewsRun>> {
    let reader = state.vault().await?.db.reader().clone();
    let limit = i64::from(limit.min(NEWS_RUNS_CAP));
    Ok(news::list_runs(&reader, limit).await?)
}

/// Здоровье эндпоинта анализатора новостей (W-39): отдаётся кнопке «Проверить связь».
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsEndpointHealth {
    /// Эндпоинт ответил (любой HTTP-статус) — провайдер достижим.
    pub ok: bool,
    /// Человеко-читаемое сообщение (RU): «доступен» / причина недоступности.
    pub message: String,
    /// Базовый URL пингованного эндпоинта (тот, что реально использует пайплайн новостей).
    pub endpoint: String,
    /// Латентность пинга в миллисекундах.
    pub latency_ms: u32,
}

/// URL провайдера, которым РЕАЛЬНО пользуются новости: пайплайн (`news::run`/`news_article`/
/// `news_summarize`) выбирает модель по `news.json::model_pref` ([`select_news_chat`]) — `ai.fast`
/// (дефолт/`"fast"`/`None`) ИЛИ `ai.chat` (`"main"`), с fallback на другую. Здесь зеркалим ТОТ ЖЕ
/// порядок по конфигу `.nexus/local.json`, чтобы «Проверить связь» (W-39) пинговала именно тот URL,
/// которым пайплайн реально пользуется при текущем выборе модели.
fn news_provider_url(cfg: &LocalConfig, pref: Option<&str>) -> Option<String> {
    let chat_url = || cfg.ai.chat.as_ref().map(|c| c.url.clone());
    let fast_url = || cfg.ai.fast.as_ref().map(|f| f.url.clone());
    let ordered = if pref == Some("main") {
        chat_url().or_else(fast_url) // "main": ai.chat, fallback ai.fast
    } else {
        fast_url().or_else(chat_url) // "fast"/None: ai.fast, fallback ai.chat (прежнее поведение)
    };
    ordered.filter(|u| !u.trim().is_empty())
}

/// W-39: пингует ПРОВАЙДЕРА новостей (анализатор записей/перевод — `ai.fast` с fallback на `ai.chat`).
/// Переиспользует тот же health-механизм, что «Проверить связь» в настройках (`GuardedClient::for_probe`
/// + `EgressFeature::Probe` + пробный GET `/v1/models`): URL проверяется политикой эгресса ДО сети
/// (первый egress-вектор закрыт), офлайн-тумблер режет, аудит пишет. Любой ответ сервера → достижим;
/// сетевая ошибка → нет (с причиной). Эндпоинт не настроен → `ok=false` с подсказкой.
#[tauri::command]
pub async fn news_test_endpoint(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<NewsEndpointHealth> {
    // W-40: пингуем URL модели, выбранной в news.json (`model_pref`) — консистентно с пайплайном.
    let model_pref = news::load_news_config(&config_path(&app)?).model_pref;
    let root = state.vault().await?.root.clone();
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .unwrap_or_default();
    let cfg = if raw.trim().is_empty() {
        LocalConfig::default()
    } else {
        LocalConfig::parse(&raw).map_err(|e| AppError::Msg(e.to_string()))?
    };
    let Some(endpoint) = news_provider_url(&cfg, model_pref.as_deref()) else {
        return Ok(NewsEndpointHealth {
            ok: false,
            message: "анализатор новостей не настроен — укажите эндпоинт в Настройки → ИИ".into(),
            endpoint: String::new(),
            latency_ms: 0,
        });
    };

    let probe = GuardedClient::for_probe(
        state.egress_policy.clone(),
        state.egress_audit.clone(),
        NEWS_PROBE_TIMEOUT,
    )
    .map_err(crate::ai::AiError::from)?;
    let target = format!("{}/v1/models", crate::ai::api_base(&endpoint));
    let started = std::time::Instant::now();
    // «Проверить связь» — вне прогона агента → RunCtx::NONE (как test_ai_connection в настройках).
    let result = probe.get(&target, EgressFeature::Probe, RunCtx::NONE).await;
    let latency_ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
    let (ok, message) = match result {
        Ok(_) => (true, "анализатор новостей доступен".to_string()),
        Err(NetError::Denied(d)) => (false, d.to_string()),
        Err(NetError::BadUrl) => (false, "некорректный URL эндпоинта".to_string()),
        Err(NetError::Http(e)) => (false, format!("эндпоинт недоступен: {e}")),
    };
    Ok(NewsEndpointHealth {
        ok,
        message,
        endpoint,
        latency_ms,
    })
}

/// Максимальный размер копируемого лог-файла (W-39, anti-DoS, зеркалит `backup::MAX_BACKUP_BYTES`):
/// журнал ротируется по дням, но защищаемся от патологически разросшегося файла ДО чтения в память.
const MAX_LOG_BYTES: u64 = crate::backup::MAX_BACKUP_BYTES as u64;

/// W-39: копирует САМЫЙ СВЕЖИЙ лог-файл из `log_dir()` в `path` (FE отдаёт путь save-диалогом — fs
/// остаётся в доверенном бэкенде, фронт не получает прав на запись; зеркало `backup_export_to_path`).
/// Полный лог (не отфильтрованный по «news:») — полезнее для дебага. Нет файлов → внятная ошибка.
#[tauri::command]
pub async fn export_news_logs(path: String) -> AppResult<()> {
    let dir = crate::log_dir().ok_or_else(|| AppError::Msg("каталог логов недоступен".into()))?;
    let src = latest_log_file(&dir).await?.ok_or_else(|| {
        AppError::Msg("файлов журнала ещё нет — действий пока не записано".into())
    })?;
    let meta = tokio::fs::metadata(&src)
        .await
        .map_err(|e| AppError::Msg(format!("лог-файл недоступен: {e}")))?;
    // Anti-DoS: отсекаем патологически разросшийся файл по размеру ДО копирования.
    if meta.len() > MAX_LOG_BYTES {
        return Err(AppError::Msg(format!(
            "лог-файл слишком большой ({} байт > предела {MAX_LOG_BYTES})",
            meta.len()
        )));
    }
    tokio::fs::copy(&src, &path)
        .await
        .map_err(|e| AppError::Msg(format!("запись файла логов: {e}")))?;
    Ok(())
}

/// Самый свежий лог-файл в каталоге (по mtime): tracing-appender ротирует по дням
/// (`nexus.log.YYYY-MM-DD`), поэтому берём не по имени, а по времени модификации — устойчиво к
/// схеме именования. `None` — каталога/файлов ещё нет.
async fn latest_log_file(dir: &std::path::Path) -> AppResult<Option<std::path::PathBuf>> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::Msg(format!("каталог логов недоступен: {e}"))),
    };
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    while let Some(entry) = rd
        .next_entry()
        .await
        .map_err(|e| AppError::Msg(format!("чтение каталога логов: {e}")))?
    {
        let meta = match entry.metadata().await {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            best = Some((mtime, entry.path()));
        }
    }
    Ok(best.map(|(_, p)| p))
}

/// Отметка прочитано/непрочитано (AC-NF-9).
#[tauri::command]
pub async fn news_mark_read(state: State<'_, AppState>, id: i64, read: bool) -> AppResult<()> {
    let writer = state.vault().await?.db.writer().clone();
    Ok(news::mark_read(&writer, id, read, crate::scheduler::now_secs()).await?)
}

/// «В заметку» (AC-NF-11): создаёт заметку `News/<дата> <заголовок>.md` с фронтматтером
/// `source`/`news_source`, RU-резюме и ссылкой на оригинал; путь уникален (повтор → суффикс).
/// Индексация — штатно watcher'ом. Возвращает относительный путь заметки.
#[tauri::command]
pub async fn news_to_note(state: State<'_, AppState>, id: i64) -> AppResult<String> {
    let (root, reader) = {
        let ctx = state.vault().await?;
        (ctx.root.clone(), ctx.db.reader().clone())
    };
    let item = news::get_item(&reader, id)
        .await?
        .ok_or_else(|| AppError::Msg(format!("запись ленты не найдена: {id}")))?;
    make_news_note(&root, &item).map_err(AppError::Msg)
}

/// Ручной прогон «Обновить» (AC-NF-6): ставит джобу kind `newsfeed` с дедупом — уже стоящая в
/// очереди/выполняющаяся не дублируется. Возвращает `true`, если джоба поставлена.
#[tauri::command]
pub async fn refresh_news(state: State<'_, AppState>) -> AppResult<bool> {
    let (writer, reader) = {
        let ctx = state.vault().await?;
        (ctx.db.writer().clone(), ctx.db.reader().clone())
    };
    let now = crate::scheduler::now_secs();
    if crate::scheduler::has_ready_job(&reader, news::KIND_NEWSFEED, now).await? {
        return Ok(false); // уже в очереди/выполняется — дедуп
    }
    crate::scheduler::enqueue(&writer, news::KIND_NEWSFEED, "", now, 2).await?;
    Ok(true)
}

/// Конфиг ленты для страницы настроек (AC-NF-9).
#[tauri::command]
pub async fn get_news_config(app: AppHandle) -> AppResult<NewsConfig> {
    Ok(news::load_news_config(&config_path(&app)?))
}

/// Источник реестра для UI (consent-строка CTA и будущие настройки источников, AC-NF-7):
/// имя + действующий вкл/выкл с учётом переопределений `news.json`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsSourceDto {
    pub id: String,
    pub title: String,
    pub enabled: bool,
    pub lang_ru: bool,
}

/// Реестр источников v1 с действующими флагами (consent показывает, КУДА пойдут запросы).
#[tauri::command]
pub async fn news_sources(app: AppHandle) -> AppResult<Vec<NewsSourceDto>> {
    let cfg = news::load_news_config(&config_path(&app)?);
    Ok(news::SOURCES_V1
        .iter()
        .map(|s| NewsSourceDto {
            id: s.id.to_string(),
            title: s.title.to_string(),
            enabled: cfg.source_enabled(s),
            lang_ru: s.lang_ru,
        })
        .collect())
}

/// Сохраняет конфиг (вкл/выкл фичи = consent, источники, ключи), СИНХРОНИЗИРУЕТ политику
/// эгресса (NF-4: тоггл `NewsFeed`-фичи + "news"-скоуп allowlist — мгновенно, AC-NF-7)
/// и возвращает применённый конфиг.
#[tauri::command]
pub async fn set_news_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: NewsConfig,
) -> AppResult<NewsConfig> {
    let path = config_path(&app)?;
    news::save_news_config(&path, &config)
        .map_err(|e| AppError::Msg(format!("news.json не записан: {e}")))?;
    news::sync_egress_policy(&state.egress_policy, &config);
    Ok(config)
}

/// Разрешает хост статьи по клику из ридера (opt-in владельца 2026-06-11, ревизия решения NF-6):
/// добавляет ПУБЛИЧНЫЙ хост в `news.json::extra_hosts` + мгновенно пересинхронизирует "news"-скоуп.
/// Гарантии: per-host (не глобальный тумблер), персист вне vault/git, снимается из gear-меню;
/// приватные/LAN-хосты отвергаются здесь же (и всё равно были бы отрезаны политикой web-класса
/// + DNS-гардом — defense-in-depth). Возвращает применённый конфиг.
#[tauri::command]
pub async fn news_allow_host(
    app: AppHandle,
    state: State<'_, AppState>,
    host: String,
) -> AppResult<NewsConfig> {
    let host = host.trim().to_lowercase();
    // Синтаксис: парсим как хост абсолютного URL — отрезает схемы/пути/порты/мусор.
    let parsed = reqwest::Url::parse(&format!("https://{host}/"))
        .ok()
        .and_then(|u| u.host_str().map(str::to_string));
    if parsed.as_deref() != Some(host.as_str()) || host.is_empty() {
        return Err(AppError::Msg(format!("некорректный хост: {host:?}")));
    }
    if crate::plugin::is_private_host(&host) {
        return Err(AppError::Msg(
            "приватные/LAN-хосты запрещены политикой эгресса (W-аддендум)".into(),
        ));
    }
    let path = config_path(&app)?;
    let mut cfg = news::load_news_config(&path);
    if !cfg.enabled {
        return Err(AppError::Msg(
            "лента выключена — включите её сначала".into(),
        ));
    }
    if !cfg.extra_hosts.contains(&host) {
        cfg.extra_hosts.push(host.clone());
        news::save_news_config(&path, &cfg)
            .map_err(|e| AppError::Msg(format!("news.json не записан: {e}")))?;
    }
    news::sync_egress_policy(&state.egress_policy, &cfg);
    tracing::info!(host = %host, "ридер: хост статьи разрешён владельцем (extra_hosts)");
    Ok(cfg)
}

/// Снимает разрешение с хоста статьи (gear-меню ленты). Идемпотентно; возвращает конфиг.
#[tauri::command]
pub async fn news_disallow_host(
    app: AppHandle,
    state: State<'_, AppState>,
    host: String,
) -> AppResult<NewsConfig> {
    let host = host.trim().to_lowercase();
    let path = config_path(&app)?;
    let mut cfg = news::load_news_config(&path);
    cfg.extra_hosts.retain(|h| h != &host);
    news::save_news_config(&path, &cfg)
        .map_err(|e| AppError::Msg(format!("news.json не записан: {e}")))?;
    news::sync_egress_policy(&state.egress_policy, &cfg);
    tracing::info!(host = %host, "ридер: разрешение хоста снято");
    Ok(cfg)
}

/// Статья для reader (NF-6). `denied` — хост вне политики эгресса (HN-ссылки на произвольные
/// домены, офлайн, выключенная фича): fail-closed; расширение allowlist — ТОЛЬКО явным per-host
/// consent (`news_allow_host`), UI показывает резюме, ссылку «Оригинал» и кнопку «Разрешить».
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum NewsArticleDto {
    #[serde(rename_all = "camelCase")]
    Ready {
        paras: Vec<String>,
        /// Текст переведён моделью (EN-источник); RU-источники не переводятся (D1).
        translated: bool,
        /// Исходник был усечён потолком символов (no silent caps — пометка в reader).
        truncated: bool,
    },
    #[serde(rename_all = "camelCase")]
    Denied { message: String },
}

/// Полный текст статьи для reader: кэш → иначе guarded-фетч оригинала (политика NF-4 как есть:
/// хост вне news-allowlist → `denied`, никакого расширения по клику) → извлечение абзацев →
/// RU-перевод утилитарной моделью → кэш в БД. Долгий вызов (LLM) — UI показывает прогресс.
#[tauri::command]
pub async fn news_article(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<NewsArticleDto> {
    // W-40: модель новостей по выбору пользователя (`news.json::model_pref`); None → fast-дефолт.
    let model_pref = news::load_news_config(&config_path(&app)?).model_pref;
    let (reader, writer, policy, audit, chat) = {
        let ctx = state.vault().await?;
        (
            ctx.db.reader().clone(),
            ctx.db.writer().clone(),
            ctx.ai.policy.clone(),
            state.egress_audit.clone(),
            select_news_chat(&ctx.ai, model_pref.as_deref()),
        )
    };
    let item = news::get_item(&reader, id)
        .await?
        .ok_or_else(|| AppError::Msg(format!("запись ленты не найдена: {id}")))?;

    // Кэш: повторное открытие без сети и LLM.
    if let Some((body, truncated)) = news::get_body(&reader, id).await? {
        return Ok(NewsArticleDto::Ready {
            paras: body.split("\n\n").map(str::to_string).collect(),
            translated: !item.lang_ru,
            truncated,
        });
    }

    // Fail-closed пре-чек политики (читаемый отказ ДО сети): статья может жить на хосте вне
    // доверенных источников (HN агрегирует произвольные домены) — это НЕ ошибка, а состояние.
    let host = reqwest::Url::parse(&item.url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .ok_or_else(|| AppError::Msg("некорректный URL записи".into()))?;
    if let Err(denied) = policy.check(&host, EgressFeature::NewsFeed) {
        return Ok(NewsArticleDto::Denied {
            message: denied.to_string(),
        });
    }

    let chat = chat.ok_or_else(|| {
        AppError::Msg("LLM не сконфигурирован — перевод статьи недоступен".into())
    })?;
    let fetcher = news::GuardedNewsFetcher::new(policy, audit, Arc::new(news::SystemResolver));
    let html = news::FeedFetcher::fetch(&fetcher, &item.url)
        .await
        .map_err(|e| AppError::Msg(format!("оригинал не загружен: {e}")))?;
    let (paras, truncated) = news::extract_paragraphs(&html);
    if paras.is_empty() {
        return Err(AppError::Msg(
            "не удалось извлечь текст статьи — откройте оригинал".into(),
        ));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let (paras, translated) =
        news::translate_article(&chat, &item.title_ru, &paras, item.lang_ru, &cancel)
            .await
            .map_err(AppError::Msg)?;
    news::set_body(
        &writer,
        id,
        paras.join("\n\n"),
        truncated,
        crate::scheduler::now_secs(),
    )
    .await?;
    Ok(NewsArticleDto::Ready {
        paras,
        translated,
        truncated,
    })
}

/// «Сократить» (NF-6): 3–6 RU-тезисов по тексту статьи (кэш тела; без него — по резюме).
#[tauri::command]
pub async fn news_summarize(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<Vec<String>> {
    // W-40: тот же выбор модели, что и в news_article — анализ и сокращение через одну модель.
    let model_pref = news::load_news_config(&config_path(&app)?).model_pref;
    let (reader, chat) = {
        let ctx = state.vault().await?;
        (
            ctx.db.reader().clone(),
            select_news_chat(&ctx.ai, model_pref.as_deref()),
        )
    };
    let chat: Arc<dyn ChatProvider> =
        chat.ok_or_else(|| AppError::Msg("LLM не сконфигурирован — сокращение недоступно".into()))?;
    let item = news::get_item(&reader, id)
        .await?
        .ok_or_else(|| AppError::Msg(format!("запись ленты не найдена: {id}")))?;
    let paras: Vec<String> = match news::get_body(&reader, id).await? {
        Some((body, _)) => body.split("\n\n").map(str::to_string).collect(),
        None if !item.summary_ru.is_empty() => vec![item.summary_ru.clone()],
        None => return Err(AppError::Msg("нет текста для сокращения".into())),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    news::summarize_article(&chat, &item.title_ru, &paras, &cancel)
        .await
        .map_err(AppError::Msg)
}

fn config_path(app: &AppHandle) -> AppResult<std::path::PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Msg(format!("config-dir недоступен: {e}")))?;
    Ok(dir.join("news.json"))
}

/// Тестируемое ядро «в заметку»: пишет файл в `News/` vault'а (анти-traversal через
/// `resolve_vault_path_for_write`), уникализирует имя суффиксом.
/// Потолок символов резюме в поисковом запросе (заголовок несёт тему; длинный summary размывает
/// центроид эмбеддинга и тянет FTS-ветку в шум). Обрезаем по СИМВОЛАМ (не байтам — кириллица).
const RELATED_QUERY_SUMMARY_CHARS: usize = 512;
/// Сколько связанных заметок по умолчанию (компактная секция ридера).
const RELATED_LIMIT_DEFAULT: usize = 6;
/// Запас перед постфильтром (self-note, floor, дедуп по файлу).
const RELATED_OVERFETCH: usize = 4;
/// Мягкий RRF-floor (search RRF_K=60): отсекает только хвост ниже ~rank-12 в одном списке. score из
/// hybrid_search — RRF (≈макс 0.0328), НЕ косинус, поэтому абсолютный 0.30 обнулил бы выдачу.
const RELATED_RRF_FLOOR: f32 = 0.012;

/// Поисковый запрос «связанных заметок» из новости: заголовок целиком + начало резюме.
fn build_related_query(title_ru: &str, summary_ru: &str) -> String {
    let summary: String = summary_ru
        .trim()
        .chars()
        .take(RELATED_QUERY_SUMMARY_CHARS)
        .collect();
    format!("{} {}", title_ru.trim(), summary.trim())
        .trim()
        .to_string()
}

/// Пути заметок, созданных ИЗ ЭТОЙ новости (frontmatter `source == url`, см. make_news_note) — чтобы не
/// показывать «связанной» саму себя (её контент = title+summary новости → сходство ~1.0). Таблица
/// `frontmatter_fields` индексируется (indexer), фильтр надёжный (не эвристик по пути/заголовку).
async fn news_self_note_paths(
    reader: &ReadPool,
    url: &str,
) -> DbResult<std::collections::HashSet<String>> {
    let url = url.to_string();
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT f.path FROM frontmatter_fields ff JOIN files f ON f.id=ff.file_id \
                 WHERE ff.key='source' AND ff.value=?1 AND f.is_deleted=0",
            )?;
            let rows = stmt.query_map([url], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<std::collections::HashSet<String>>>()
        })
        .await
}

/// FLOW: «Связанные заметки» к новости — семантический поиск по vault (hybrid_search по тексту
/// заголовок+резюме). Вторичный discovery-аффорданс в ридере: лениво, без кэша. НЕТ векторного индекса
/// → пусто (секция скрыта). Контент новости (перевод недоверенного фида) идёт ТОЛЬКО в embed_query/
/// FTS-токены (НЕ в LLM-промпт) — prompt-injection невозможна, FTS токенизирует+экранирует.
/// EGRESS: запрос эмбеддится тем же guarded-каналом [`EgressFeature::Embed`], что и поиск/чат-RAG —
/// при УДАЛЁННОМ эмбеддере (прод bge:8083) текст новости уходит на embed-хост по сети; это покрыто
/// существующей политикой эгресса (офлайн-тумблер режет, audit пишет), отдельного opt-in нет by
/// design. НОВОГО egress-surface не добавляет — иной встроенный host/feature тут не появляется.
#[tauri::command]
pub async fn news_related(
    state: State<'_, AppState>,
    id: i64,
    limit: Option<usize>,
) -> AppResult<Vec<LinkSuggestion>> {
    let (reader, vectors, embedder) = {
        let ctx = state.vault().await?;
        (
            ctx.db.reader().clone(),
            ctx.vectors.clone(),
            ctx.ai.embedder.clone(),
        )
    };
    // Валидируем запись ДО degrade-на-vectors — иначе невалидный id давал бы Err с RAG и Ok([]) без
    // (несогласованность, пойманная adversarial-ревью): теперь not-found = Err независимо от индекса.
    let item = news::get_item(&reader, id)
        .await?
        .ok_or_else(|| AppError::Msg(format!("запись ленты не найдена: {id}")))?;
    let Some(vectors) = vectors else {
        return Ok(Vec::new()); // нет RAG-индекса → секция скрыта (вторичный аффорданс)
    };
    let limit = limit.unwrap_or(RELATED_LIMIT_DEFAULT).min(20);
    Ok(related_notes(&reader, vectors.as_ref(), embedder.as_deref(), &item, limit).await?)
}

/// Ядро `news_related` без `State` — тестируемо напрямую. Запрос из новости → hybrid_search →
/// RRF-floor + self-note-фильтр + дедуп по файлу. `vectors` обязателен (degrade на None — в команде).
async fn related_notes(
    reader: &ReadPool,
    vectors: &crate::vector::VectorIndex,
    embedder: Option<&dyn crate::ai::EmbeddingProvider>,
    item: &NewsItem,
    limit: usize,
) -> DbResult<Vec<LinkSuggestion>> {
    let query = build_related_query(&item.title_ru, &item.summary_ru);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let opts = SearchOptions {
        limit: limit * RELATED_OVERFETCH, // запас под self/floor/дедуп-по-файлу
        filter: None,
        center: None, // новость не файл в графе — граф-ранг неприменим
    };
    let hits = search::hybrid_search(reader, Some(vectors), embedder, query, opts).await?;
    let self_paths = news_self_note_paths(reader, &item.url).await?;
    let mut seen = std::collections::HashSet::new();
    Ok(hits
        .into_iter()
        .filter(|h| h.score >= RELATED_RRF_FLOOR) // хвост по RRF-floor
        .filter(|h| !self_paths.contains(&h.path)) // не показываем заметку-из-этой-новости
        .filter(|h| seen.insert(h.path.clone())) // одна карточка на файл (max score — он сверху)
        .take(limit)
        .map(|h| LinkSuggestion {
            path: h.path,
            title: h.title,
            score: h.score, // сырой RRF; фронт его НЕ показывает (% на RRF бессмысленны)
            reason: h.snippet,
        })
        .collect())
}

fn make_news_note(root: &std::path::Path, item: &NewsItem) -> Result<String, String> {
    std::fs::create_dir_all(root.join("News")).map_err(|e| e.to_string())?;
    let date = unix_to_date(item.published_at.max(0));
    let slug = note_slug(&item.title_ru);
    let mut rel = format!("News/{date} {slug}.md");
    let mut n = 1;
    while root.join(&rel).exists() {
        n += 1;
        rel = format!("News/{date} {slug} {n}.md");
    }
    let abs = crate::vault::resolve_vault_path_for_write(root, std::path::Path::new(&rel))
        .map_err(|e| e.to_string())?;
    let content = format!(
        "---\nsource: {url}\nnews_source: {src}\n---\n\n# {title}\n\n{summary}\n\n[Оригинал]({url})\n",
        url = item.url,
        src = item.source_id,
        title = item.title_ru,
        summary = item.summary_ru,
    );
    crate::vault::atomic_write_io(&abs, content.as_bytes()).map_err(|e| e.to_string())?; // атомарно (аудит)
    Ok(rel)
}

/// Имя файла из RU-заголовка: убираем запрещённые ФС-символы, схлопываем пробелы, ≤60 символов.
fn note_slug(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '[' | ']' => ' ',
            c => c,
        })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = joined.chars().take(60).collect();
    let out = trimmed.trim().to_string();
    if out.is_empty() {
        "Новость".to_string()
    } else {
        out
    }
}

/// Unix-секунды → `YYYY-MM-DD` (обратный алгоритм Хиннанта; без chrono, как `days_from_civil`).
fn unix_to_date(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Мок chat-провайдера для тестов выбора модели: несёт лишь идентификатор, чтобы убедиться,
    /// КАКОЙ слот (`chat_util`/`chat_fast`) выбрал [`select_news_chat`] по `model_pref`.
    struct TagProvider(&'static str);
    #[async_trait::async_trait]
    impl ChatProvider for TagProvider {
        async fn stream_chat(
            &self,
            _messages: &[crate::ai::ChatMessage],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
        ) -> crate::ai::AiResult<String> {
            Ok(String::new())
        }
        fn model_id(&self) -> &str {
            self.0
        }
    }

    /// Собирает `AIClient` с заданными слотами утилитарной/основной модели (остальное — None/пусто).
    fn ai_client(util: Option<&'static str>, fast: Option<&'static str>) -> crate::ai::AIClient {
        crate::ai::AIClient {
            chat: None,
            chat_fast: fast.map(|m| Arc::new(TagProvider(m)) as Arc<dyn ChatProvider>),
            chat_util: util.map(|m| Arc::new(TagProvider(m)) as Arc<dyn ChatProvider>),
            embedder: None,
            agent_tools: None,
            policy: Arc::new(crate::net::EgressPolicy::new(Arc::new(AtomicBool::new(
                false,
            )))),
        }
    }

    /// W-40: `select_news_chat` — дефолт/`"fast"`/`None` берёт утилитарную (`chat_util`), `"main"` —
    /// основную (`chat_fast`); при отсутствии выбранной модели падает на другую (fallback).
    #[test]
    fn select_news_chat_routes_by_pref() {
        let ai = ai_client(Some("util-model"), Some("main-model"));

        // None / "fast" / неизвестное → утилитарная (ТЕКУЩЕЕ поведение, 0 регрессии).
        for pref in [None, Some("fast"), Some("bogus")] {
            assert_eq!(
                select_news_chat(&ai, pref).unwrap().model_id(),
                "util-model",
                "pref={pref:?} → утилитарная"
            );
        }
        // "main" → основная.
        assert_eq!(
            select_news_chat(&ai, Some("main")).unwrap().model_id(),
            "main-model"
        );

        // Fallback: "main" без основной → утилитарная; "fast"/None без утилитарной → основная.
        let only_util = ai_client(Some("util-model"), None);
        assert_eq!(
            select_news_chat(&only_util, Some("main"))
                .unwrap()
                .model_id(),
            "util-model"
        );
        let only_fast = ai_client(None, Some("main-model"));
        assert_eq!(
            select_news_chat(&only_fast, None).unwrap().model_id(),
            "main-model"
        );

        // Обе пусты → None независимо от pref.
        let empty = ai_client(None, None);
        assert!(select_news_chat(&empty, Some("main")).is_none());
        assert!(select_news_chat(&empty, None).is_none());
    }

    /// W-40: `news_provider_url` зеркалит тот же порядок выбора по `model_pref` для «Проверить связь».
    #[test]
    fn news_provider_url_mirrors_pref() {
        let cfg = LocalConfig::parse(
            r#"{"ai":{"chat":{"url":"http://chat:8080"},"fast":{"url":"http://fast:8084"}}}"#,
        )
        .unwrap();
        // None / fast → ai.fast url (прежнее поведение).
        assert_eq!(
            news_provider_url(&cfg, None).as_deref(),
            Some("http://fast:8084")
        );
        assert_eq!(
            news_provider_url(&cfg, Some("fast")).as_deref(),
            Some("http://fast:8084")
        );
        // main → ai.chat url.
        assert_eq!(
            news_provider_url(&cfg, Some("main")).as_deref(),
            Some("http://chat:8080")
        );

        // Fallback при отсутствии выбранного эндпоинта.
        let only_chat =
            LocalConfig::parse(r#"{"ai":{"chat":{"url":"http://chat:8080"}}}"#).unwrap();
        assert_eq!(
            news_provider_url(&only_chat, None).as_deref(),
            Some("http://chat:8080"),
            "fast не задан → fallback на chat"
        );
        let only_fast =
            LocalConfig::parse(r#"{"ai":{"fast":{"url":"http://fast:8084"}}}"#).unwrap();
        assert_eq!(
            news_provider_url(&only_fast, Some("main")).as_deref(),
            Some("http://fast:8084"),
            "main без chat → fallback на fast"
        );

        // Ничего не настроено → None.
        let empty = LocalConfig::default();
        assert!(news_provider_url(&empty, Some("main")).is_none());
    }

    fn item(title_ru: &str) -> NewsItem {
        NewsItem {
            id: 1,
            source_id: "openai".into(),
            url: "https://example.com/post".into(),
            title_ru: title_ru.into(),
            summary_ru: "Короткое резюме.".into(),
            topic: "Модели".into(),
            lang_ru: false,
            published_at: 1_780_000_000, // 2026-05-28
            read: false,
            comments_url: None,
        }
    }

    /// AC-NF-11: заметка с фронтматтером source/news_source, RU-контентом и ссылкой; повтор →
    /// уникальный суффикс; слэши/решётки из заголовка не ломают путь (анти-traversal цел).
    #[test]
    fn makes_unique_note_with_frontmatter() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();

        let rel = make_news_note(&root, &item("Claude 5: что нового / обзор")).unwrap();
        assert!(rel.starts_with("News/2026-05-28 Claude 5"), "{rel}");
        let body = std::fs::read_to_string(root.join(&rel)).unwrap();
        assert!(body.starts_with("---\nsource: https://example.com/post\n"));
        assert!(body.contains("news_source: openai"));
        // В теле — оригинальный заголовок (слаг чистится только для имени файла).
        assert!(body.contains("# Claude 5: что нового / обзор"));
        assert!(body.contains("[Оригинал](https://example.com/post)"));

        let rel2 = make_news_note(&root, &item("Claude 5: что нового / обзор")).unwrap();
        assert_ne!(rel, rel2, "повтор → суффикс");
        assert!(rel2.ends_with(" 2.md"));

        // Попытка traversal через заголовок гасится заменой символов.
        let evil = make_news_note(&root, &item("../../etc/passwd")).unwrap();
        assert!(evil.starts_with("News/"));
        assert!(root.join(&evil).exists());
    }

    /// Дата заметки из published_at; вырожденные заголовки не дают пустое имя.
    #[test]
    fn date_and_slug_edge_cases() {
        assert_eq!(unix_to_date(0), "1970-01-01");
        assert_eq!(unix_to_date(1_780_000_000), "2026-05-28");
        assert_eq!(note_slug("///"), "Новость");
        assert_eq!(note_slug(&"д".repeat(100)).chars().count(), 60);
    }

    #[test]
    fn build_related_query_concats_and_truncates_by_chars() {
        assert_eq!(
            build_related_query("Заголовок", "Резюме."),
            "Заголовок Резюме."
        );
        assert_eq!(build_related_query("  T  ", "  S  "), "T S");
        assert_eq!(build_related_query("", ""), "");
        // Обрезка резюме по СИМВОЛАМ (не байтам) — кириллица не рвётся, UTF-8 валиден.
        let long = "я".repeat(600);
        let q = build_related_query("T", &long);
        assert!(q.chars().count() <= 2 + RELATED_QUERY_SUMMARY_CHARS); // "T " + ≤512
        assert!(std::str::from_utf8(q.as_bytes()).is_ok());
    }

    /// FLOW: related_notes отдаёт релевантную заметку и ОТФИЛЬТРОВЫВАЕТ заметку-из-этой-новости
    /// (frontmatter source==url) — иначе новость «связана сама с собой» (контент почти 1.0).
    #[tokio::test]
    async fn related_notes_ranks_relevant_and_filters_self_note() {
        use crate::ai::{EmbeddingProvider, MockEmbedder};
        use crate::db::Database;
        use crate::indexer::Indexer;
        use crate::vector::VectorIndex;

        let dir = TempDir::new().unwrap();
        // canonicalize — иначе на macOS TempDir под симлинком /var→/private/var, и make_news_note
        // (resolve_vault_path_for_write) сочтёт путь traversal'ом (как makes_unique_note_with_frontmatter).
        let root = dir.path().canonicalize().unwrap();
        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors.usearch"), 16).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
        let idx = Indexer::with_rag(&db, root.clone(), embedder.clone(), vectors.clone(), true);

        // Релевантная (общие слова с новостью) и нерелевантная заметки.
        std::fs::write(
            root.join("rag.md"),
            "Модели и эмбеддинги в RAG-пайплайне важны для семантического поиска.",
        )
        .unwrap();
        std::fs::write(
            root.join("bread.md"),
            "Рецепт хлеба на закваске с изюмом и мёдом по бабушкиному совету.",
        )
        .unwrap();
        idx.index_file("rag.md").await.unwrap();
        idx.index_file("bread.md").await.unwrap();

        let mut it = item("Новые модели и эмбеддинги для RAG");
        it.summary_ru = "Обзор RAG-пайплайнов и эмбеддингов современных моделей.".into();

        // Заметка, созданная ИЗ этой новости (frontmatter source == it.url) — должна быть отфильтрована.
        let self_rel = make_news_note(&root, &it).unwrap();
        idx.index_file(&self_rel).await.unwrap();

        let out = related_notes(
            db.reader(),
            vectors.as_ref(),
            Some(embedder.as_ref()),
            &it,
            5,
        )
        .await
        .unwrap();
        let paths: Vec<&str> = out.iter().map(|s| s.path.as_str()).collect();
        assert!(
            paths.contains(&"rag.md"),
            "релевантная заметка в выдаче: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.starts_with("News/")),
            "заметка-из-этой-новости (source==url) отфильтрована: {paths:?}"
        );
    }

    /// Пустой запрос (нет заголовка и резюме) → пусто без обращения к поиску.
    #[tokio::test]
    async fn related_notes_empty_query_is_empty() {
        use crate::ai::MockEmbedder;
        use crate::db::Database;
        use crate::vector::VectorIndex;

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors.usearch"), 16).unwrap());
        let embedder = MockEmbedder { dim: 16 };
        let mut it = item("");
        it.summary_ru = "".into();
        let out = related_notes(db.reader(), vectors.as_ref(), Some(&embedder), &it, 5)
            .await
            .unwrap();
        assert!(out.is_empty());
    }
}
