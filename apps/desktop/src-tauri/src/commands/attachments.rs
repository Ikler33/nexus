//! Картинки в заметках (IMG-1): запись вставленной/перетащенной картинки в `attachments/` и чтение
//! её как `data:`-URL для превью. Подход — **data-URL** (CSP уже разрешает `data:`): БЕЗ включения
//! asset-протокола и БЕЗ изменения CSP/capabilities. Безопасность: имя вложения — только basename без
//! сепараторов/`..`/dot-префикса и с image-расширением; запись через `resolve_vault_path_for_write`,
//! чтение через `resolve_vault_path` (анти-traversal); лимит размера 20 МБ; SVG читается как картинка
//! (в контексте `<img>` скрипты в SVG не исполняются).

use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine};
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::vault;

/// Папка вложений в vault.
const ATTACH_DIR: &str = "attachments";
/// Потолок размера вложения (защита от OOM на огромном файле).
const MAX_BYTES: usize = 20 * 1024 * 1024;

/// MIME по расширению имени (None → не считаем картинкой).
fn mime_for_ext(name: &str) -> Option<&'static str> {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        "ico" => Some("image/x-icon"),
        _ => None,
    }
}

/// Резолвит относительный путь вложения → канонический абсолютный путь ВНУТРИ vault, отвергая служебные
/// директории (`.nexus`/`.git`/dot/`.conflict`) — В ТОМ ЧИСЛЕ через симлинк (проверка на канонизированном
/// пути). Ревью IMG-EMBED: иначе `![[notes/lnk.png]]`, где `lnk.png → ../.nexus/secret.png`, утёк бы
/// содержимое `.nexus` (паритет с дот-гардом `is_pinnable`/permission). `None` — вне vault / не существует
/// (resolve_vault_path канонизирует, требуя существования) / служебное.
fn safe_attachment_abs(root: &Path, rel: &str) -> Option<std::path::PathBuf> {
    let abs = vault::resolve_vault_path(root, Path::new(rel)).ok()?;
    if crate::watcher::is_ignored(&abs) {
        return None;
    }
    Some(abs)
}

/// Безопасное имя вложения: непустой basename ≤120 символов, без `/ \ ..`, без dot-префикса,
/// с допустимым image-расширением. (Запись всё равно идёт через resolve_vault_path_for_write.)
fn is_safe_attachment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && !name.starts_with('.')
        && mime_for_ext(name).is_some()
}

/// Копия vault-root (лок отпускаем сразу).
async fn vault_root(state: &State<'_, AppState>) -> AppResult<std::path::PathBuf> {
    Ok(state.vault().await?.root.clone())
}

/// Пишет картинку-вложение `attachments/<name>` из base64. Возвращает относительный путь для `![](…)`.
#[tauri::command]
pub async fn write_attachment(
    state: State<'_, AppState>,
    name: String,
    data_base64: String,
) -> AppResult<String> {
    if !is_safe_attachment_name(&name) {
        return Err(AppError::Msg("недопустимое имя вложения".into()));
    }
    let bytes = STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|_| AppError::Msg("повреждённые данные вложения".into()))?;
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(AppError::Msg("вложение пустое или слишком большое".into()));
    }
    let root = vault_root(&state).await?;
    let rel = format!("{ATTACH_DIR}/{name}");
    let abs = vault::resolve_vault_path_for_write(&root, Path::new(&rel))?;
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent).await?; // attachments/ может ещё не существовать
    }
    tokio::task::spawn_blocking(move || vault::atomic_write(&abs, &bytes))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))??;
    Ok(rel)
}

/// Резолвит цель картинки-вставки `![[name]]` → относительный путь vault (для `read_attachment`).
/// `name` с сепаратором — явный относительный путь (анти-traversal + проверка существования);
/// голый basename — обход vault за картинкой с таким именем (как basename-шорткат `[[ссылок]]`:
/// КРАТЧАЙШИЙ путь, регистронезависимо), пропуская служебные папки. Картинки НЕ в индексе (`files`
/// только .md), поэтому резолв — обход ФС, read-only. `None` — не image-расширение / не найдено.
#[tauri::command]
pub async fn resolve_attachment(
    state: State<'_, AppState>,
    name: String,
) -> AppResult<Option<String>> {
    if mime_for_ext(&name).is_none() {
        return Ok(None); // не картинка по расширению
    }
    let root = vault_root(&state).await?;
    // Явный путь (есть сепаратор): анти-traversal + анти-служебное (.nexus/симлинк) + существование —
    // всё в safe_attachment_abs (canonicalize требует существования). basename-обход не нужен.
    if name.contains('/') || name.contains('\\') {
        return Ok(safe_attachment_abs(&root, &name).map(|_| name.replace('\\', "/")));
    }
    // Голый basename: обход (read_dir не блокирует async-рантайм надолго на типичном vault, но всё же
    // в spawn_blocking — на больших vault обход синхронный).
    let found = tokio::task::spawn_blocking(move || find_image_by_basename(&root, &name))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?;
    Ok(found)
}

/// Обход vault за картинкой по basename (регистронезависимо) → КРАТЧАЙШИЙ относительный путь (при
/// коллизии выигрывает ближе к корню, как basename-шорткат ссылок). Пропускает служебные папки
/// (`is_ignored`: `.nexus`/`.git`/dot/`.conflict`). Чистая (root + имя) — тестируется на TempDir.
fn find_image_by_basename(root: &Path, basename: &str) -> Option<String> {
    let want = basename.to_ascii_lowercase();
    let mut best: Option<String> = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if crate::watcher::is_ignored(&path) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
                if fname.to_ascii_lowercase() == want && mime_for_ext(fname).is_some() {
                    if let Ok(stripped) = path.strip_prefix(root) {
                        let rel = stripped.to_string_lossy().replace('\\', "/");
                        // Кратчайший путь выигрывает (≤ оставляет уже найденный при равной длине).
                        match &best {
                            Some(b) if b.len() <= rel.len() => {}
                            _ => best = Some(rel),
                        }
                    }
                }
            }
        }
    }
    best
}

/// Читает вложение по относительному пути → `data:<mime>;base64,…` для отображения в превью.
#[tauri::command]
pub async fn read_attachment(state: State<'_, AppState>, path: String) -> AppResult<String> {
    let Some(mime) = mime_for_ext(&path) else {
        return Err(AppError::Msg("не картинка".into()));
    };
    let root = vault_root(&state).await?;
    // Ревью IMG-EMBED: гард служебных папок (`.nexus`/`.git`, в т.ч. через симлинк) на канонизированном
    // пути — read_attachment был достижим напрямую (`![](.nexus/x.png)` через VaultImage) и утёк бы файл.
    let Some(abs) = safe_attachment_abs(&root, &path) else {
        return Err(AppError::Msg("вложение недоступно".into()));
    };
    let bytes = tokio::fs::read(&abs).await?;
    if bytes.len() > MAX_BYTES {
        return Err(AppError::Msg("вложение слишком большое".into()));
    }
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::{
        find_image_by_basename, is_safe_attachment_name, mime_for_ext, safe_attachment_abs,
    };
    use std::fs;

    /// Готовит TempDir-vault: создаёт перечисленные относительные файлы (с промежуточными папками).
    fn vault_with(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for rel in files {
            let abs = dir.path().join(rel);
            fs::create_dir_all(abs.parent().unwrap()).unwrap();
            fs::write(&abs, b"x").unwrap();
        }
        dir
    }

    #[test]
    fn basename_resolves_in_subfolder() {
        let v = vault_with(&["attachments/pic.png", "Notes/idea.md"]);
        assert_eq!(
            find_image_by_basename(v.path(), "pic.png"),
            Some("attachments/pic.png".into())
        );
    }

    #[test]
    fn basename_is_case_insensitive() {
        let v = vault_with(&["attachments/Pic.PNG"]);
        assert_eq!(
            find_image_by_basename(v.path(), "pic.png"),
            Some("attachments/Pic.PNG".into())
        );
    }

    #[test]
    fn collision_prefers_shortest_path() {
        let v = vault_with(&["deep/sub/dir/logo.png", "logo.png"]);
        // оба basename совпадают → ближе к корню (короче путь) выигрывает.
        assert_eq!(
            find_image_by_basename(v.path(), "logo.png"),
            Some("logo.png".into())
        );
    }

    #[test]
    fn ignores_service_dirs() {
        let v = vault_with(&[".nexus/secret.png", ".git/x.png"]);
        // картинки в служебных папках не всплывают basename-обходом.
        assert_eq!(find_image_by_basename(v.path(), "secret.png"), None);
        assert_eq!(find_image_by_basename(v.path(), "x.png"), None);
    }

    #[test]
    fn missing_basename_is_none() {
        let v = vault_with(&["attachments/pic.png"]);
        assert_eq!(find_image_by_basename(v.path(), "nope.png"), None);
    }

    #[test]
    fn non_image_basename_not_matched() {
        let v = vault_with(&["Notes/note.md"]);
        // даже если есть файл с таким именем, не-картинка не матчится (mime_for_ext отсекает).
        assert_eq!(find_image_by_basename(v.path(), "note.md"), None);
    }

    // Ревью IMG-EMBED — гард служебных папок в явном пути (safe_attachment_abs):
    #[test]
    fn explicit_legit_attachment_resolves() {
        let v = vault_with(&["attachments/pic.png"]);
        let root = v.path().canonicalize().unwrap();
        assert!(safe_attachment_abs(&root, "attachments/pic.png").is_some());
    }

    #[test]
    fn explicit_service_dir_rejected() {
        let v = vault_with(&[".nexus/secret.png"]);
        let root = v.path().canonicalize().unwrap();
        // прямой путь в .nexus — отвергнут (паритет с basename-обходом и is_pinnable).
        assert_eq!(safe_attachment_abs(&root, ".nexus/secret.png"), None);
    }

    #[test]
    fn explicit_traversal_and_missing_rejected() {
        let v = vault_with(&["attachments/pic.png"]);
        let root = v.path().canonicalize().unwrap();
        assert_eq!(safe_attachment_abs(&root, "../escape.png"), None);
        assert_eq!(safe_attachment_abs(&root, "attachments/nope.png"), None);
    }

    // MAJOR ревью IMG-EMBED: симлинк ВНУТРИ vault, указывающий в .nexus — канонизация ведёт в .nexus,
    // is_ignored на канон-пути обязан отвергнуть (иначе утечка содержимого .nexus). Только unix (симлинки).
    #[cfg(unix)]
    #[test]
    fn explicit_symlink_into_service_dir_rejected() {
        let v = vault_with(&[".nexus/secret.png"]);
        fs::create_dir_all(v.path().join("notes")).unwrap();
        std::os::unix::fs::symlink("../.nexus/secret.png", v.path().join("notes/lnk.png")).unwrap();
        let root = v.path().canonicalize().unwrap();
        assert_eq!(safe_attachment_abs(&root, "notes/lnk.png"), None);
    }

    #[test]
    fn mime_by_ext() {
        assert_eq!(mime_for_ext("a.png"), Some("image/png"));
        assert_eq!(mime_for_ext("a.JPG"), Some("image/jpeg"));
        assert_eq!(mime_for_ext("a.jpeg"), Some("image/jpeg"));
        assert_eq!(mime_for_ext("a.webp"), Some("image/webp"));
        assert_eq!(mime_for_ext("a.svg"), Some("image/svg+xml"));
        assert_eq!(mime_for_ext("a.ico"), Some("image/x-icon"));
        assert_eq!(mime_for_ext("a.txt"), None);
        assert_eq!(mime_for_ext("noext"), None);
    }

    #[test]
    fn safe_names_accepted_unsafe_rejected() {
        assert!(is_safe_attachment_name("pasted-123.png"));
        assert!(is_safe_attachment_name("Screenshot.jpeg"));
        // traversal / сепараторы / служебные / не-картинки — отклоняются
        assert!(!is_safe_attachment_name("../secret.png"));
        assert!(!is_safe_attachment_name("a/b.png"));
        assert!(!is_safe_attachment_name("a\\b.png"));
        assert!(!is_safe_attachment_name(".hidden.png"));
        assert!(!is_safe_attachment_name("note.md"));
        assert!(!is_safe_attachment_name("evil.png.exe"));
        assert!(!is_safe_attachment_name(""));
    }
}
