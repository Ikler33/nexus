//! Команды ленты новостей (NF-3, AC-NF-9): страница читает ленту/темы/последний прогон,
//! отмечает прочитанное, сохраняет «в заметку» (AC-NF-11), дёргает ручной прогон и правит
//! конфиг (`news.json` в OS config-dir — consent-носитель, AC-NF-7). Сам прогон гоняет
//! планировщик (kind `newsfeed`); регистрация хендлера с реальным фетчером — срез NF-4.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::news::{self, NewsConfig, NewsItem, NewsRun};
use crate::state::AppState;

/// Размер страницы ленты (карточек за запрос) — без безлимитных выгрузок (урок #22).
const PAGE_SIZE: i64 = 50;

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

fn config_path(app: &AppHandle) -> AppResult<std::path::PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Msg(format!("config-dir недоступен: {e}")))?;
    Ok(dir.join("news.json"))
}

/// Тестируемое ядро «в заметку»: пишет файл в `News/` vault'а (анти-traversal через
/// `resolve_vault_path_for_write`), уникализирует имя суффиксом.
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
    std::fs::write(&abs, content).map_err(|e| e.to_string())?;
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
}
