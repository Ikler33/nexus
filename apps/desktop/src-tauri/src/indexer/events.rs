//! Watcher-петля индексатора: спавн фоновой задачи (начальный скан → реакция на события vault) и
//! Tauri-эвент «индекс обновлён» для живого пересчёта зависимых вьюх (ADR-007 S8).

use crate::watcher::{VaultEvent, VaultWatcher};

use super::fs::rel_of;
use super::Indexer;

/// Запускает watcher + фоновый цикл индексации для готового `Indexer` (вызывается из `open_vault`,
/// который решает, с RAG или без). Watcher живёт внутри спавненной задачи; на завершении — стоп.
pub fn spawn(indexer: Indexer, app: tauri::AppHandle) {
    let root = indexer.root.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VaultEvent>();
    let watcher = match VaultWatcher::new(&root, tx) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "vault watcher init failed");
            return;
        }
    };
    tokio::spawn(async move {
        let _watcher = watcher; // держим watcher живым на время задачи
        if let Err(e) = indexer.scan_vault().await {
            tracing::error!(error = %e, "initial vault scan failed");
        }
        emit_vault_changed(&app); // индекс готов после начального скана
        while let Some(event) = rx.recv().await {
            let result = match event {
                VaultEvent::Upsert(abs) => match rel_of(&indexer.root, &abs) {
                    Some(rel) => indexer.index_file(&rel).await,
                    None => Ok(()),
                },
                VaultEvent::Deleted(abs) => match rel_of(&indexer.root, &abs) {
                    Some(rel) => indexer.remove_file(&rel).await,
                    None => Ok(()),
                },
                VaultEvent::Renamed { from, to } => {
                    match (rel_of(&indexer.root, &from), rel_of(&indexer.root, &to)) {
                        (Some(from_rel), Some(to_rel)) => {
                            indexer.rename_file(&from_rel, &to_rel).await
                        }
                        // Перемещение из/в пределы vault → как удаление/создание соответственно.
                        (None, Some(to_rel)) => indexer.index_file(&to_rel).await,
                        (Some(from_rel), None) => indexer.remove_file(&from_rel).await,
                        (None, None) => Ok(()),
                    }
                }
            };
            match result {
                // Персистим usearch после каждого инкрементального события (события дебаунсятся
                // watcher'ом, не на каждое нажатие). Дебаунс самого save — позже при росте индекса.
                Ok(()) => {
                    indexer.persist_vectors();
                    emit_vault_changed(&app); // живой пересчёт зависимых вьюх (ADR-007 S8, #35 «Цели»)
                }
                Err(e) => tracing::warn!(error = %e, "index event failed"),
            }
        }
    });
}

/// Tauri-событие «индекс vault обновлён» (backend→фронт, ADR-007 S8 — event-канал планировщика). Фронт
/// по нему перечитывает зависимые от индекса вьюхи (напр. «Цели» #35, AC-GP-3). Best-effort: ошибка
/// emit (нет окна и т.п.) не критична.
fn emit_vault_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit("vault:changed", ());
}
