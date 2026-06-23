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
