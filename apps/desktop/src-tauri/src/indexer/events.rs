//! Watcher-петля индексатора: спавн фоновой задачи (начальный скан → реакция на события vault) и
//! Tauri-эвент «индекс обновлён» для живого пересчёта зависимых вьюх (ADR-007 S8).

use serde::Serialize;

use crate::watcher::{VaultEvent, VaultWatcher};

/// Payload события `vault:index-progress` (camelCase для фронта).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexProgress {
    done: usize,
    total: usize,
}

/// Payload события `vault:file-changed` (SAFE-3): относительный путь + blake3-хеш текущего диска.
/// Фронт сверяет хеш с `Buffer.baseHash`: совпал → эхо своего сейва (игнор); расходится → тихий
/// reload (чистый буфер) либо баннер guard'а (грязный буфер). camelCase для фронта.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileChanged {
    path: String,
    hash: String,
}

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
    // Прогресс полного скана → событие фронту (статусбар «Индексация N/M», макет app.jsx).
    let progress_app = app.clone();
    let indexer = indexer.with_progress(move |done, total| {
        use tauri::Emitter;
        let _ = progress_app.emit("vault:index-progress", IndexProgress { done, total });
    });
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<VaultEvent>();
    let watcher = match VaultWatcher::new(&root, tx.clone()) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "vault watcher init failed");
            return None;
        }
    };
    let changed_app = app.clone();
    tokio::spawn(event_loop(
        indexer,
        rx,
        move || emit_vault_changed(&app),
        move |path, hash| emit_file_changed(&changed_app, path, hash),
    ));
    Some((watcher, tx))
}

/// Петля индексации: начальный скан → инкрементальные события до закрытия канала (= дропа
/// watcher'а из `VaultContext`). `notify` — хук «индекс обновлён» (в проде Tauri-эвент; вынесен,
/// чтобы петля тестировалась без `AppHandle`).
pub(super) async fn event_loop(
    indexer: Indexer,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<VaultEvent>,
    notify: impl Fn() + Send + 'static,
    on_file_changed: impl Fn(String, String) + Send + 'static,
) {
    if let Err(e) = indexer.scan_vault().await {
        tracing::error!(error = %e, "initial vault scan failed");
    }
    notify(); // индекс готов после начального скана
    while let Some(event) = rx.recv().await {
        let result = match event {
            VaultEvent::Upsert(abs) => match rel_of(&indexer.root, &abs) {
                Some(rel) => {
                    let r = indexer.index_file(&rel).await;
                    // SAFE-3: per-file сигнал «файл на диске изменился» + хеш диска. ТОЛЬКО
                    // инкрементальный путь вотчера (начальный скан/Rescan идут через scan_vault
                    // мимо этой ветки → шторма событий нет). Эхо своего сейва глушит фронт
                    // (hash == baseHash). Промах чтения (гонка с удалением) → без события.
                    if r.is_ok() {
                        if let Ok(bytes) = tokio::fs::read(&abs).await {
                            on_file_changed(rel.clone(), crate::vault::content_hash(&bytes));
                        }
                    }
                    r
                }
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

/// Tauri-событие «конкретный файл на диске изменился» (SAFE-3, backend→фронт). Фронт по нему
/// решает судьбу открытого буфера этого пути (эхо своего сейва / тихий reload / баннер guard'а).
/// Best-effort: ошибка emit (нет окна) не критична.
fn emit_file_changed(app: &tauri::AppHandle, path: String, hash: String) {
    use tauri::Emitter;
    let _ = app.emit("vault:file-changed", FileChanged { path, hash });
}
