//! Watcher-петля индексатора: спавн фоновой задачи (начальный скан → реакция на события vault) и
//! инъектируемые хуки «индекс обновлён» для живого пересчёта зависимых вьюх (ADR-007 S8).
//!
//! CORE-1c-1: модуль отвязан от Tauri. Раньше [`spawn`] принимал Tauri `AppHandle` и сам строил
//! Tauri-эвенты (`vault:changed` / `vault:file-changed` / `vault:index-progress`). Теперь он
//! принимает [`IndexerHooks`] — набор колбэков к окружению вызывающего; desktop-крейт строит их из
//! `AppHandle::emit(...)` на месте проводки (`commands::vault::open_vault`). Зеркалит паттерн
//! `scheduler::WorkerHooks` (CORE-1b): генерик-петля tauri-free и тестируется без `AppHandle`.

use std::sync::Arc;

use crate::watcher::{VaultEvent, VaultWatcher};

use super::fs::rel_of;
use super::Indexer;

/// Хуки watcher-петли индексатора к окружению вызывающего (вынесены, чтобы петля была tauri-free и
/// тестировалась без `AppHandle`). Desktop строит их из `AppHandle::emit(...)`:
/// - `on_progress` → событие `vault:index-progress` (статусбар «Индексация N/M»);
/// - `on_vault_changed` → событие `vault:changed` (живой пересчёт зависимых вьюх, ADR-007 S8);
/// - `on_file_changed(rel, hash)` → событие `vault:file-changed` (SAFE-3: судьба открытого буфера).
///
/// `Arc` — колбэки шарятся между прогресс-хуком индексатора и петлёй (оба `'static`, Send+Sync).
#[derive(Clone)]
pub struct IndexerHooks {
    /// Прогресс полного скана (done, total) — для статусбара. Best-effort у вызывающего.
    pub on_progress: Arc<dyn Fn(usize, usize) + Send + Sync>,
    /// «Индекс vault обновлён» — UI перечитывает зависимые от индекса вьюхи. Best-effort.
    pub on_vault_changed: Arc<dyn Fn() + Send + Sync>,
    /// «Конкретный файл на диске изменился» (rel-путь + blake3-хеш диска) — SAFE-3. Best-effort.
    pub on_file_changed: Arc<dyn Fn(String, String) + Send + Sync>,
}

/// Запускает watcher + фоновый цикл индексации для готового `Indexer` (вызывается из `open_vault`,
/// который решает, с RAG или без). `hooks` инъектирует эмит-колбэки окружения (desktop строит их из
/// `AppHandle`); ядро/headless могут передать no-op или собственные стоки.
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
    hooks: IndexerHooks,
) -> Option<(VaultWatcher, tokio::sync::mpsc::UnboundedSender<VaultEvent>)> {
    let root = indexer.root.clone();
    // Прогресс полного скана → колбэк окружению (статусбар «Индексация N/M», макет app.jsx).
    let on_progress = hooks.on_progress.clone();
    let indexer = indexer.with_progress(move |done, total| on_progress(done, total));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<VaultEvent>();
    let watcher = match VaultWatcher::new(&root, tx.clone()) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "vault watcher init failed");
            return None;
        }
    };
    let on_vault_changed = hooks.on_vault_changed.clone();
    let on_file_changed = hooks.on_file_changed.clone();
    tokio::spawn(event_loop(
        indexer,
        rx,
        move || on_vault_changed(),
        move |path, hash| on_file_changed(path, hash),
    ));
    Some((watcher, tx))
}

/// Петля индексации: начальный скан → инкрементальные события до закрытия канала (= дропа
/// watcher'а из `VaultContext`). `notify` — хук «индекс обновлён» (в проде Tauri-эвент через
/// [`IndexerHooks`]; вынесен, чтобы петля тестировалась без `AppHandle`).
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
