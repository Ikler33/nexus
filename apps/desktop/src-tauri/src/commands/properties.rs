//! Команды реестра типов свойств (PROP-2): чтение/правка `.nexus/property-types.json` (тип глобален по
//! имени, Obsidian Properties). Эвристика по значению — в `properties::infer_type` (используется на чтении
//! заметки в PROP-3). Офлайн, без LLM.

use std::collections::BTreeMap;
use std::path::Path;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::properties::{self, NoteProperty, PropertyType};
use crate::state::AppState;
use crate::vault;

/// Весь реестр типов свойств (имя → явный тип). Без открытого vault — ошибка.
#[tauri::command]
pub async fn get_property_types(
    state: State<'_, AppState>,
) -> AppResult<BTreeMap<String, PropertyType>> {
    let root = state.vault().await?.root.clone();
    tokio::task::spawn_blocking(move || properties::load(&root))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))
}

/// Задаёт ЯВНЫЙ тип свойства `key` (меняет глобально по имени, как Obsidian). Пустое имя — ошибка.
#[tauri::command]
pub async fn set_property_type(
    state: State<'_, AppState>,
    key: String,
    ty: PropertyType,
) -> AppResult<()> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(AppError::Msg("пустое имя свойства".into()));
    }
    let root = state.vault().await?.root.clone();
    tokio::task::spawn_blocking(move || {
        let mut reg = properties::load(&root);
        reg.insert(key, ty);
        properties::save(&root, &reg)
    })
    .await
    .map_err(|e| AppError::Msg(e.to_string()))?
    .map_err(AppError::Io)?;
    Ok(())
}

/// Свойства заметки (PROP-3 Properties-панель): плоские frontmatter-скаляры с разрешённым типом-виджетом
/// (реестр+эвристика). Порядок — как в файле. Без открытого vault — ошибка.
#[tauri::command]
pub async fn get_note_properties(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<Vec<NoteProperty>> {
    let root = state.vault().await?.root.clone();
    let abs = vault::resolve_vault_path(&root, Path::new(&path))?;
    let content = tokio::fs::read_to_string(&abs).await?;
    tokio::task::spawn_blocking(move || {
        let reg = properties::load(&root);
        let fields = crate::parser::parse(&content).fields;
        properties::note_properties(&reg, &fields)
    })
    .await
    .map_err(|e| AppError::Msg(e.to_string()))
}
