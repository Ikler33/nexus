//! Открытие внешних ссылок в СИСТЕМНОМ браузере (NF-6 «Оригинал», web-источники чата, ссылки в
//! превью заметок). В Tauri-вебвью `<a target="_blank">` не открывает браузер: строгий CSP
//! (`default-src 'self'`) глотает навигацию. Открываем host-side через `tauri-plugin-opener`.
//!
//! Безопасность: контент новостей/веба НЕДОВЕРЕННЫЙ — пускаем ТОЛЬКО `http`/`https`, отсекая
//! `file:`/кастомные схемы (вектор запуска чужого приложения). Это НЕ эгресс приложения (фетчит
//! ОС-браузер, не мы) → `net::GuardedClient` не задействован; схема-гард — единственный контроль.
//! Команда дёргает плагин host-side, поэтому отдельная capability `opener:*` не нужна.

use tauri_plugin_opener::OpenerExt;

/// Открывает `url` в системном браузере по умолчанию. Отклоняет любую схему, кроме http/https.
#[tauri::command]
pub fn open_external(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let parsed = reqwest::Url::parse(&url).map_err(|e| e.to_string())?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("схема не разрешена: {other}")),
    }
    app.opener()
        .open_url(parsed.as_str(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    /// Схема-гард: только http/https проходят парс+проверку (логика выделена для юнита без AppHandle).
    fn scheme_ok(url: &str) -> bool {
        reqwest::Url::parse(url)
            .map(|u| matches!(u.scheme(), "http" | "https"))
            .unwrap_or(false)
    }

    #[test]
    fn only_http_https_allowed() {
        assert!(scheme_ok("https://github.com/x/y"));
        assert!(scheme_ok("http://news.ycombinator.com/item?id=1"));
        assert!(!scheme_ok("file:///etc/passwd"));
        assert!(!scheme_ok("javascript:alert(1)"));
        assert!(!scheme_ok("mailto:a@b.c"));
        assert!(!scheme_ok("not a url"));
    }
}
