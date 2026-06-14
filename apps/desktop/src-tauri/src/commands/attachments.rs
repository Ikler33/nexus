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
        _ => None,
    }
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

/// Читает вложение по относительному пути → `data:<mime>;base64,…` для отображения в превью.
#[tauri::command]
pub async fn read_attachment(state: State<'_, AppState>, path: String) -> AppResult<String> {
    let Some(mime) = mime_for_ext(&path) else {
        return Err(AppError::Msg("не картинка".into()));
    };
    let root = vault_root(&state).await?;
    let abs = vault::resolve_vault_path(&root, Path::new(&path))?;
    let bytes = tokio::fs::read(&abs).await?;
    if bytes.len() > MAX_BYTES {
        return Err(AppError::Msg("вложение слишком большое".into()));
    }
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::{is_safe_attachment_name, mime_for_ext};

    #[test]
    fn mime_by_ext() {
        assert_eq!(mime_for_ext("a.png"), Some("image/png"));
        assert_eq!(mime_for_ext("a.JPG"), Some("image/jpeg"));
        assert_eq!(mime_for_ext("a.jpeg"), Some("image/jpeg"));
        assert_eq!(mime_for_ext("a.webp"), Some("image/webp"));
        assert_eq!(mime_for_ext("a.svg"), Some("image/svg+xml"));
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
