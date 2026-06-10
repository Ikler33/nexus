//! Watcher-петля индексатора: спавн фоновой задачи (начальный скан → реакция на события vault) и
//! Tauri-эвент «индекс обновлён» для живого пересчёта зависимых вьюх (ADR-007 S8).

use crate::watcher::{VaultEvent, VaultWatcher};

use super::fs::rel_of;
use super::Indexer;

/// Запускает watcher + фоновый цикл индексации для готового `Indexer` (вызывается из `open_vault`,
/// который решает, с RAG или без).
///
/// **Владение watcher'ом — у ВЫЗЫВАЮЩЕГО** (фикс «вечных воркеров», аудит 2026-06-10): возвращаемые
/// `VaultWatcher` + sender живут в `VaultContext`; замена контекста (повторный `open_vault`)
/// дропает оба → канал закрывается → [`event_loop`] выходит сам. Раньше watcher жил
/// ВНУТРИ задачи → петля была вечной, и каждый `open_vault` плодил ещё одну (два watcher'а на
/// каталог, двойная индексация). `None` — watcher не инициализировался (vault без живой
/// индексации, как и раньше).
///
/// Sender — управляющий вход той же петли: команда `rescan_vault` шлёт [`VaultEvent::Rescan`]
/// (ручной реиндекс сериализуется с fs-событиями, без второго конкурентного сканера).
#[must_use = "watcher обязан жить в VaultContext::lifecycle — иначе петля индексации умрёт сразу"]
pub fn spawn(
    indexer: Indexer,
    app: tauri::AppHandle,
) -> Option<(VaultWatcher, tokio::sync::mpsc::UnboundedSender<VaultEvent>)> {
    let root = indexer.root.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<VaultEvent>();
    let watcher = match VaultWatcher::new(&root, tx.clone()) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "vault watcher init failed");
            return None;
        }
    };
    tokio::spawn(event_loop(indexer, rx, move || emit_vault_changed(&app)));
    Some((watcher, tx))
}

/// Петля индексации: начальный скан → инкрементальные события до закрытия канала (= дропа
/// watcher'а из `VaultContext`). `notify` — хук «индекс обновлён» (в проде Tauri-эвент; вынесен,
/// чтобы петля тестировалась без `AppHandle`).
pub(super) async fn event_loop(
    indexer: Indexer,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<VaultEvent>,
    notify: impl Fn() + Send + 'static,
) {
    if let Err(e) = indexer.scan_vault().await {
        tracing::error!(error = %e, "initial vault scan failed");
    }
    notify(); // индекс готов после начального скана
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
                    (Some(from_rel), Some(to_rel)) => indexer.rename_file(&from_rel, &to_rel).await,
                    // Перемещение из/в пределы vault → как удаление/создание соответственно.
                    (None, Some(to_rel)) => indexer.index_file(&to_rel).await,
                    (Some(from_rel), None) => indexer.remove_file(&from_rel).await,
                    (None, None) => Ok(()),
                }
            }
            // Ручной реиндекс (`rescan_vault`): тот же полный обход, что на открытии vault
            // (mtime-шорткат внутри index_file — неизменённые файлы пролетают быстро).
            VaultEvent::Rescan => indexer.scan_vault().await,
        };
        match result {
            // Персистим usearch после каждого инкрементального события (события дебаунсятся
            // watcher'ом, не на каждое нажатие). Дебаунс самого save — позже при росте индекса.
            Ok(()) => {
                indexer.persist_vectors();
                notify(); // живой пересчёт зависимых вьюх (ADR-007 S8, #35 «Цели»)
            }
            Err(e) => tracing::warn!(error = %e, "index event failed"),
        }
    }
    tracing::debug!("indexer event loop stopped (vault закрыт/заменён)");
}

/// Tauri-событие «индекс vault обновлён» (backend→фронт, ADR-007 S8 — event-канал планировщика). Фронт
/// по нему перечитывает зависимые от индекса вьюхи (напр. «Цели» #35, AC-GP-3). Best-effort: ошибка
/// emit (нет окна и т.п.) не критична.
fn emit_vault_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit("vault:changed", ());
}
