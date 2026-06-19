//! Планировщик фоновых задач — **app-specific glue** (CORE-1b). Обобщённый движок (очередь+диспатч+
//! воркер-луп через [`WorkerHooks`]) живёт в `nexus_core::scheduler` и ре-экспортится отсюда целиком,
//! так что `crate::scheduler::enqueue`/`Registry`/`now_secs`/… по всему приложению резолвятся без
//! изменений. Здесь — то, что движок (tauri-free) знать не может:
//! - [`WorkerSpawner`] (держит `tauri::AppHandle`) + `start()`: строит `WorkerHooks` из `AppState`/
//!   `AppHandle` и спавнит супервизор поверх `nexus_core::scheduler::worker_loop`;
//! - [`WorkerHandle`] — хендл живого воркера (shutdown-канал + abort супервизора), хранится в lifecycle;
//! - [`emit_jobs_changed`] — Tauri-событие `jobs:changed` для StatusBar;
//! - встроенный kind [`KIND_GC`] + `GcHandler` (чистит `done` + кэши `contradictions`/`relation_reasons`
//!   — APP-модули) + [`default_registry`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

// Обобщённый движок планировщика — ре-экспорт целиком: `crate::scheduler::{enqueue, Registry,
// JobHandler, WorkerHooks, worker_loop, run_due, requeue_running, now_secs, Job, JobCounts, …}`
// продолжают резолвиться по всему приложению с минимальной правкой импортов.
pub use nexus_core::scheduler::*;

use nexus_core::db::WriteActor;

/// Пауза перед рестартом умершего воркера (супервизор) — не молотим при систематической ошибке.
/// (Движок свои тайминги держит сам; это — тайминг супервизора-glue.)
const RESPAWN_DELAY_SECS: u64 = 5;

/// Конфигурация воркера планировщика: все (клонируемо-дешёвые) зависимости в одном месте, чтобы
/// супервизор можно было перезапустить заново (N1, ручной «Перезапустить фоновые задачи» из UI —
/// поднимаем воркер без переоткрытия vault). `start()` поднимает супервизор и отдаёт хендл.
/// Держит `tauri::AppHandle` (отсюда строятся [`WorkerHooks`]) → остаётся в app-glue, а не в движке.
#[derive(Clone)]
pub struct WorkerSpawner {
    pub writer: WriteActor,
    pub app: tauri::AppHandle,
    pub registry: Arc<Registry>,
    pub recurring: HashMap<String, i64>,
    pub reader: nexus_core::db::ReadPool,
    pub on_change: Vec<String>,
}

/// Хендл живого воркера (хранится в `VaultContext::lifecycle`): shutdown-sender (дроп → штатный
/// стоп, Drop-семантика как раньше) + abort-хендл супервизора (для явного перезапуска).
pub struct WorkerHandle {
    /// Дроп sender'а гасит цикл (changed()→Err→break). Живёт в lifecycle.
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// Принудительная остановка супервизора (при перезапуске — рвём старый до старта нового).
    pub supervisor: tokio::task::AbortHandle,
}

impl WorkerSpawner {
    /// Поднимает супервизор воркера со свежим shutdown-каналом. Возвращает хендл для хранения/рестарта.
    pub fn start(&self) -> WorkerHandle {
        use tauri::Manager;
        let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let cfg = self.clone();
        // Супервизор (инцидент 2026-06-12: воркер «тихо исчез» без паники — ready-джобы стояли 13ч):
        // неожиданное завершение цикла (паника/return) → ERROR с причиной и РЕСТАРТ через паузу.
        // Штатный выход (shutdown/дроп sender) — стоп.
        let supervisor = tokio::spawn(async move {
            loop {
                let app2 = cfg.app.clone();
                let app3 = cfg.app.clone();
                let hooks = WorkerHooks {
                    interactive_busy: Box::new(move || {
                        app2.state::<crate::state::AppState>().is_interactive_busy()
                    }),
                    jobs_changed: Box::new(move || emit_jobs_changed(&app3)),
                };
                let handle = tokio::spawn(worker_loop(
                    cfg.writer.clone(),
                    cfg.registry.clone(),
                    cfg.recurring.clone(),
                    cfg.reader.clone(),
                    cfg.on_change.clone(),
                    hooks,
                    shutdown.clone(),
                ));
                match handle.await {
                    Ok(()) => {
                        tracing::info!("scheduler worker stopped (shutdown)");
                        break;
                    }
                    Err(join_err) => {
                        tracing::error!(error = %join_err, "scheduler worker УМЕР — рестарт супервизором");
                    }
                }
                if *shutdown.borrow() || shutdown.has_changed().is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(RESPAWN_DELAY_SECS)).await;
            }
        });
        WorkerHandle {
            shutdown: shutdown_tx,
            supervisor: supervisor.abort_handle(),
        }
    }
}

/// Tauri-событие «состояние очереди изменилось» (для StatusBar N/M — срез UI). Best-effort.
fn emit_jobs_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit("jobs:changed", ());
}

// ── встроенный kind + реестр по умолчанию (app-glue: GcHandler чистит APP-кэши) ───────────────────

/// Встроенный kind «gc»: периодическая чистка завершённых джоб (S7). Первый live-потребитель воркера.
pub const KIND_GC: &str = "gc";
/// Сколько хранить `done`-джобы до сборки мусора.
const GC_RETENTION_SECS: i64 = 7 * 24 * 3600;

/// Обработчик «gc»: удаляет `done`-джобы старше retention + самоочистка кэша противоречий
/// (CT-3+ хвост: пары по удалённым/переименованным заметкам). Держит свой клон write-actor.
/// Остаётся в app-glue: зовёт APP-модули `crate::contradictions`/`crate::relation_reasons`.
struct GcHandler {
    writer: WriteActor,
    retention_secs: i64,
}

#[async_trait]
impl JobHandler for GcHandler {
    async fn handle(&self, _job: &Job) -> Result<(), String> {
        gc_done(&self.writer, now_secs() - self.retention_secs)
            .await
            .map_err(|e| e.to_string())?;
        // Дёшево (один DELETE по NOT IN живых путей), работает и без сконфигурированного AI.
        let stale = crate::contradictions::gc_stale_cache(&self.writer)
            .await
            .map_err(|e| e.to_string())?;
        if stale > 0 {
            tracing::info!(stale, "gc: вычищен contradiction_cache по мёртвым путям");
        }
        // AIP-10: кэш объяснений связей — тот же приём (выметаем пары с удалённой/переименованной заметкой).
        let stale_rel = crate::relation_reasons::gc_stale_cache(&self.writer)
            .await
            .map_err(|e| e.to_string())?;
        if stale_rel > 0 {
            tracing::info!(stale_rel, "gc: вычищен relation_reasons по мёртвым путям");
        }
        Ok(())
    }
}

/// Реестр встроенных обработчиков (сейчас — только `gc`). Расширяется реальными kind (Карта/Противоречия)
/// в следующих срезах. `writer` — клон для обработчиков, которым нужна запись.
pub fn default_registry(writer: WriteActor) -> Registry {
    let mut reg = Registry::new();
    reg.insert(
        KIND_GC.to_string(),
        Arc::new(GcHandler {
            writer,
            retention_secs: GC_RETENTION_SECS,
        }),
    );
    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::db::Database;
    use tempfile::TempDir;

    async fn open() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// Встроенный kind `gc` зарегистрирован в `default_registry`, прогоняется воркером и завершается
    /// успешно (done, без retry/dead) — проверяет диспатч встроенного обработчика (app-glue).
    #[tokio::test]
    async fn gc_kind_registered_and_runs() {
        let (_d, db) = open().await;
        let w = db.writer();
        let reg = default_registry(w.clone());
        assert!(reg.contains_key(KIND_GC), "gc зарегистрирован");

        enqueue(w, KIND_GC, "", 0, 3).await.unwrap();
        assert_eq!(
            run_due(w, &reg, now_secs(), false, &HashMap::new())
                .await
                .unwrap(),
            1,
            "gc-джоба обработана"
        );
        assert!(
            claim_next(w, now_secs() + 1).await.unwrap().is_none(),
            "gc завершилась (done), не ушла в retry/dead"
        );
    }
}
