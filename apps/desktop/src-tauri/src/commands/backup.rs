//! Команды backup/restore (#59): экспорт durable «второго мозга» в JSON-строку (фронт сохраняет в
//! файл) и импорт обратно с дедупом. Чистый bridge поверх `nexus_core::backup` — вся логика в ядре.

use tauri::State;

use crate::backup;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Экспортирует полный бэкап (факты/переписка/эпизоды/телеметрия агент-скиллов) в pretty-JSON.
/// Фронт сохраняет результат в файл `*.json`. Версия приложения штампуется из `CARGO_PKG_VERSION`.
#[tauri::command]
pub async fn backup_export_json(state: State<'_, AppState>) -> AppResult<String> {
    let reader = state.vault().await?.db.reader().clone();
    let envelope = backup::export_backup(&reader, env!("CARGO_PKG_VERSION")).await?;
    serde_json::to_string_pretty(&envelope)
        .map_err(|e| AppError::Msg(format!("сериализация бэкапа: {e}")))
}

/// Импортирует бэкап из JSON-строки с дедупом (атомарно). Чужой/битый формат, слишком большой файл
/// или бэкап новее приложения → ошибка БЕЗ записи. Возвращает отчёт (что добавлено/пропущено) для UI.
#[tauri::command]
pub async fn backup_import_json(
    state: State<'_, AppState>,
    json: String,
) -> AppResult<backup::ImportReport> {
    // Anti-DoS: отсекаем гигантский/битый файл ДО материализации serde (один поток-писатель — узкое горло).
    if json.len() > backup::MAX_BACKUP_BYTES {
        return Err(AppError::Msg(format!(
            "файл бэкапа слишком большой ({} байт > предела {})",
            json.len(),
            backup::MAX_BACKUP_BYTES
        )));
    }
    let envelope: backup::BackupEnvelope =
        serde_json::from_str(&json).map_err(|e| AppError::Msg(format!("разбор бэкапа: {e}")))?;
    let writer = state.vault().await?.db.writer().clone();
    backup::import_backup(&writer, envelope)
        .await?
        .map_err(|err| AppError::Msg(err.to_string()))
}

/// W-9: экспорт бэкапа сразу в ФАЙЛ по пути из save-диалога фронта (fs остаётся в доверенном
/// бэкенде — фронт не получает прав на запись). Путь выбирает пользователь явным OS-диалогом.
#[tauri::command]
pub async fn backup_export_to_path(state: State<'_, AppState>, path: String) -> AppResult<()> {
    let reader = state.vault().await?.db.reader().clone();
    let envelope = backup::export_backup(&reader, env!("CARGO_PKG_VERSION")).await?;
    let json = serde_json::to_string_pretty(&envelope)
        .map_err(|e| AppError::Msg(format!("сериализация бэкапа: {e}")))?;
    tokio::fs::write(&path, json)
        .await
        .map_err(|e| AppError::Msg(format!("запись файла бэкапа: {e}")))?;
    Ok(())
}

/// W-9: импорт бэкапа из ФАЙЛА по пути из open-диалога фронта. Та же дедуп-логика + лимит размера,
/// что и `backup_import_json`.
#[tauri::command]
pub async fn backup_import_from_path(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<backup::ImportReport> {
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| AppError::Msg(format!("файл бэкапа недоступен: {e}")))?;
    // Anti-DoS: отсекаем гигантский файл по размеру ДО чтения в память.
    if meta.len() as usize > backup::MAX_BACKUP_BYTES {
        return Err(AppError::Msg(format!(
            "файл бэкапа слишком большой ({} байт > предела {})",
            meta.len(),
            backup::MAX_BACKUP_BYTES
        )));
    }
    let json = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| AppError::Msg(format!("чтение файла бэкапа: {e}")))?;
    let envelope: backup::BackupEnvelope =
        serde_json::from_str(&json).map_err(|e| AppError::Msg(format!("разбор бэкапа: {e}")))?;
    let writer = state.vault().await?.db.writer().clone();
    backup::import_backup(&writer, envelope)
        .await?
        .map_err(|err| AppError::Msg(err.to_string()))
}
