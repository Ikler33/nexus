//! Тривиальный no-op kind планировщика для headless-агента: доказывает, что воркер-луп ядра тикает
//! и диспатчит джобы БЕЗ app-приватных хендлеров. App-овский `default_registry`/`GcHandler` зовут
//! приватные модули десктопа (contradictions/relation_reasons), поэтому здесь — собственный минимум.

use nexus_core::scheduler::{Job, JobHandler};

/// Kind health-джобы (skeleton): успешно завершается, ничего не делает (диагностика воркер-лупа).
pub const KIND_HEALTH: &str = "health";

/// No-op обработчик: всегда `Ok` (джоба уходит в `done`, не в retry/dead) — пульс воркера.
pub struct HealthHandler;

#[async_trait::async_trait]
impl JobHandler for HealthHandler {
    async fn handle(&self, _job: &Job) -> Result<(), String> {
        tracing::debug!("health: тик воркера (no-op)");
        Ok(())
    }
}
