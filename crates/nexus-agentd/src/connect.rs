//! AF_UNIX-хостинг коннектора агента (AGENT-CONNECT P0b-2c), отделён от wiring `main.rs` (R-11).
//! Unix-only (AF_UNIX). Default-OFF: спавнится лишь при `NEXUS_AGENTD_CONNECT_SOCKET`.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use nexus_core::ai::AIClient;
use nexus_core::db::Database;

/// AF_UNIX-хостинг коннектора (AGENT-CONNECT P0b-2c), **default-OFF**. Включается env-переменной
/// `NEXUS_AGENTD_CONNECT_SOCKET=<путь>` → спавнит `serve_unix_at` поверх [`nexus_core::agent::ConnectDeps`]
/// с ТЕМИ ЖЕ зависимостями, что и `AgentRunHandler` (провайдер `ai.agent_tools` / память / актуатор-конфиг
/// / скиллы) — клонируем доли (Arc/Clone) ДО передачи остального в хендлер. Нет провайдера
/// (`ai.agent_tools=None`) → НЕ стартуем (агенту нечем думать). Автономия коннектора — параметр
/// `autonomy` (из `ai.agent_autonomy`, default `confirm`; headless-сервер может поднять до `auto`,
/// owner-gated). Unix-only.
#[allow(clippy::too_many_arguments)]
pub(crate) fn maybe_spawn_connect_server(
    db: &Database,
    ai_client: &Arc<AIClient>,
    memory: &Arc<dyn nexus_core::agent::AgentMemory>,
    canon_root: &Path,
    actuator_enabled: bool,
    autonomy: &str,
    overwrite_threshold: usize,
    blast_cap: u32,
    context_window: Option<usize>,
    loop_bounds: nexus_core::agent::LoopBounds,
    skills: &Option<nexus_core::agent::SkillContext>,
    web: &Option<nexus_core::agent::WebToolsConfig>,
    skills_learning_enabled: bool,
    delegation: &nexus_core::ai::DelegationConfig,
    research: &nexus_core::ai::ResearchConfig,
    agent_paused: &Arc<AtomicBool>,
) {
    let socket = match std::env::var("NEXUS_AGENTD_CONNECT_SOCKET") {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return, // default-OFF: env не задан → коннектор не поднимаем
    };
    let Some(provider) = ai_client.agent_tools.clone() else {
        tracing::warn!(
            "agent-connect: NEXUS_AGENTD_CONNECT_SOCKET задан, но ai.agent_tools=None — \
             коннектор НЕ поднят (нет tool-провайдера)"
        );
        return;
    };
    let deps = Arc::new(nexus_core::agent::ConnectDeps {
        provider,
        memory: Some(memory.clone()),
        writer: db.writer().clone(),
        reader: db.reader().clone(),
        canon_root: canon_root.to_path_buf(),
        actuator_enabled,
        autonomy: autonomy.to_string(),
        overwrite_threshold,
        blast_cap,
        context_window,
        loop_bounds, // BF-1: границы прогона из конфига (ai.agent_wall_clock_secs/ai.agent_max_steps)
        skills: skills.clone(),
        web: web.clone(), // EGR-AGENT-2: те же веб-инструменты, что у scheduler-AgentRunHandler
        skills_learning_enabled, // SL-7d: owner-gated авторство навыков (ai.skills.learning_enabled)
        delegation: delegation.clone(), // SUB-3b-2b: owner-gated делегирование (ai.delegation)
        research: research.clone(), // RES-5: owner-gated deep-research (ai.research)
        agent_paused: agent_paused.clone(), // ТОТ ЖЕ kill-switch, что у AgentRunHandler (SIGUSR1/agent.json)
    });
    tracing::warn!(
        socket = %socket,
        actuator_enabled,
        autonomy,
        "agent-connect: AF_UNIX коннектор ВКЛ (default-OFF; задан NEXUS_AGENTD_CONNECT_SOCKET)"
    );
    // T8: ожидаемый peer контрол-сокета = ОПЕРАТОР (= uid этого процесса agentd), НЕ run_as контейнера.
    // Linux → Some(getuid()) (fail-closed SO_PEERCRED-гейт); не-Linux → None (perms-only fallback).
    let expected_uid = nexus_core::agent::connect::operator_uid();
    tokio::spawn(async move {
        if let Err(e) = nexus_core::agent::connect::serve_unix_at(&socket, deps, expected_uid).await
        {
            tracing::error!(error = %e, "agent-connect: AF_UNIX сервер упал");
        }
    });
}
