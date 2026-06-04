//! Локальный crash-reporter (Ф4-14, §12-C / §11). Panic-hook пишет **scrubbed**-отчёт в локальный
//! файл — для диагностики на фазе ручного тестирования. **Без сети и без контента заметок**
//! (privacy by default): домашний путь заменяется на `~`, в отчёте только сообщение паники + место +
//! время. Отправка на бэкенд — строго opt-in, отдельно (BACKLOG, нужен эндпоинт + согласие).

use std::io::Write;
use std::path::PathBuf;

/// Домашний каталог (HOME / USERPROFILE), если задан и непустой.
fn home() -> Option<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|h| !h.is_empty())
}

/// Каталог крэш-логов `~/.nexus/crashes/` (создаётся при необходимости).
fn crash_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(home()?).join(".nexus").join("crashes");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Скрабит чувствительное из строки отчёта (AC-SEC-6): домашний путь → `~` (логин/структура ФС не
/// утекают), затем остальные АБСОЛЮТНЫЕ пути (вне дома — напр. vault на другом диске/маунте) →
/// `<path>/<basename>` (структура каталогов скрыта, имя файла оставлено для диагностики).
/// Относительные пути (как `src/x.rs:10` в panic-локации) и `~/…` не трогаются.
fn scrub(s: &str) -> String {
    let s = match home() {
        Some(h) => s.replace(&h, "~"),
        None => s.to_string(),
    };
    scrub_abs_paths(&s)
}

/// Сворачивает абсолютные пути (`/a/b/…`, ≥2 сегментов, начинающиеся на границе слова) в
/// `<path>/<basename>`. HOME уже заменён на `~` выше → сюда попадают только пути ВНЕ дома.
fn scrub_abs_paths(s: &str) -> String {
    // `~` тоже путь-символ → `~/vault/note` (дом) не считается «началом абсолютного пути».
    let is_path_char = |c: char| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '~');
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        // Начало абсолютного пути: '/' на границе слова (предыдущий символ — не путь-символ).
        if chars[i] == '/' && (i == 0 || !is_path_char(chars[i - 1])) {
            let start = i;
            while i < chars.len() && is_path_char(chars[i]) {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            if run.matches('/').count() >= 2 {
                let base = run.rsplit('/').find(|p| !p.is_empty()).unwrap_or("");
                out.push_str("<path>/");
                out.push_str(base);
            } else {
                out.push_str(&run); // `/usr` и подобное (1 сегмент) — не путь-к-файлу, оставляем
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Форматирует scrubbed-отчёт паники.
fn format_report(payload: &str, location: &str, ts: u64) -> String {
    format!(
        "Nexus panic\nversion: {}\ntime(unix): {ts}\nat: {}\nmessage: {}\n",
        env!("CARGO_PKG_VERSION"),
        scrub(location),
        scrub(payload),
    )
}

/// Ставит panic-hook: при панике пишет scrubbed-отчёт в `~/.nexus/crashes/crash-<ts>.log` и зовёт
/// прежний hook (дефолтный stderr-вывод сохраняется). Вызывать один раз при старте (`run`).
pub fn install_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "—".to_string());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let report = format_report(&payload, &location, ts);
        if let Some(dir) = crash_dir() {
            if let Ok(mut f) = std::fs::File::create(dir.join(format!("crash-{ts}.log"))) {
                let _ = f.write_all(report.as_bytes());
            }
        }
        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_replaces_home_and_report_has_no_raw_home() {
        // SAFETY (тест однопоточный): фиксируем HOME для детерминизма scrub.
        unsafe {
            std::env::set_var("HOME", "/Users/secretlogin");
        }
        assert_eq!(
            scrub("paniced at /Users/secretlogin/vault/note.md"),
            "paniced at ~/vault/note.md"
        );
        let report = format_report("boom", "/Users/secretlogin/src/x.rs:10:5", 42);
        assert!(
            !report.contains("secretlogin"),
            "домашний путь должен быть вычищен"
        );
        assert!(report.contains("~/src/x.rs:10:5"));
        assert!(report.contains("message: boom"));
    }

    /// V4.2: абсолютные пути ВНЕ дома (vault на другом диске/маунте) сворачиваются —
    /// структура каталогов скрыта, имя файла оставлено; относительные/`~`-пути не трогаются.
    #[test]
    fn scrub_collapses_absolute_paths_outside_home() {
        unsafe {
            std::env::set_var("HOME", "/Users/secretlogin");
        }
        let out = scrub("crash at /mnt/data/Vault/sub/secret.md here");
        assert!(
            !out.contains("/mnt/data/Vault"),
            "структура каталогов скрыта"
        );
        assert!(!out.contains("/sub/"));
        assert!(
            out.contains("<path>/secret.md"),
            "имя файла сохранено: {out}"
        );
        assert!(out.contains("crash at") && out.contains("here"));

        // Относительная panic-локация и однокорневой `/usr` — не трогаются.
        assert_eq!(
            scrub_abs_paths("at src/eval/mod.rs:10:5"),
            "at src/eval/mod.rs:10:5"
        );
        assert_eq!(scrub_abs_paths("/usr only"), "/usr only");
        // Дом (`~/…`) сохраняет структуру (имя пользователя уже вычищено выше).
        assert_eq!(scrub_abs_paths("~/vault/note.md"), "~/vault/note.md");
    }
}
