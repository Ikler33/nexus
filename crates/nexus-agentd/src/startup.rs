//! Стартовые хелперы agentd: разбор env/argv (лог-уровень, vault-путь), резолв app-config-dir и RESTORE
//! персистентных owner-kill-switch'ей (`egress.json` / `agent.json`) + рантайм-тоггл паузы по SIGUSR1.
//! Отделено от wiring `main.rs` (R-11) — самодостаточная стартовая логика с локальными тестами.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use nexus_core::net::{EgressFeature, EgressPolicy};

/// Сегмент каталога bundle-id, под которым ОБА kill-switch'а (egress.json И agent.json) живут в OS
/// config-dir. ЕДИНЫЙ источник истины (AGENT-5): дублировался бы в каждом резолве config-dir, и при
/// ребрендинге легко рассинхронизировать headless-чтение с десктоп-записью. Держим в ОДНОЙ константе —
/// ребрендинг меняет одно место.
///
/// ⚠️ ОБЯЗАН СОВПАДАТЬ с Tauri `identifier` в `tauri.conf.json` (`app.nexus.desktop`): десктоп пишет
/// `egress.json`/`agent.json` в `<OS config-dir>/<identifier>`, а headless читает их из
/// `<OS config-dir>/NEXUS_BUNDLE_DIR`. Если identifier поменяется, а эта строка — нет, headless будет
/// читать ДРУГОЙ файл, чем пишет десктоп → kill-switch'и владельца (offline / пауза агента) молча
/// неэффективны. См. [`egress_config_dir`].
const NEXUS_BUNDLE_DIR: &str = "app.nexus.desktop";

/// Грубый разбор `RUST_LOG` в `LevelFilter` (без env-filter-зависимостей). Неизвестное → info.
pub(crate) fn log_level_from_env() -> tracing::level_filters::LevelFilter {
    use tracing::level_filters::LevelFilter;
    match std::env::var("RUST_LOG")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        "off" => LevelFilter::OFF,
        _ => LevelFilter::INFO,
    }
}

/// Источник vault: `argv[1]` приоритетнее, иначе env `NEXUS_VAULT`. Ясная ошибка, если не задан.
pub(crate) fn vault_path_from_args() -> Result<PathBuf, String> {
    if let Some(arg) = std::env::args().nth(1) {
        return Ok(PathBuf::from(arg));
    }
    if let Ok(env) = std::env::var("NEXUS_VAULT") {
        if !env.is_empty() {
            return Ok(PathBuf::from(env));
        }
    }
    Err("укажите путь к vault: `nexus-agentd <vault>` или env NEXUS_VAULT".to_string())
}

/// Каталог app-local конфигов (где живёт `egress.json`) — зеркало того, что десктоп получает из Tauri
/// `app_config_dir` (`<OS config-dir>/<identifier>`). Порядок: env `NEXUS_CONFIG_DIR` (явное
/// переопределение / тесты) → `<dirs::config_dir>/app.nexus.desktop` (тот же файл, что пишет десктоп) →
/// `None`, если OS config-dir не определён (тогда kill-switch грузить неоткуда — local-first-дефолты).
///
/// ## КОНТРАКТ (AGENT-3e Fix-4) — kill-switch должен читать ТОТ ЖЕ файл, что пишет десктоп
/// Разрешённый каталог ОБЯЗАН совпадать с Tauri-десктопным `app_config_dir`. Десктоп пишет `egress.json`
/// (и `agent.json`) в `<OS config-dir>/<bundle identifier>` — а identifier берётся из `tauri.conf.json`
/// (`app.nexus.desktop`). Здесь сегмент берётся из ЕДИНОЙ константы [`NEXUS_BUNDLE_DIR`] (де-дуп —
/// AGENT-5), которая ОБЯЗАНА совпадать с тем identifier. ЕСЛИ identifier в конфиге десктопа изменится
/// (ребрендинг/смена bundle id), а [`NEXUS_BUNDLE_DIR`] — нет, headless будет читать ДРУГОЙ файл, чем
/// пишет десктоп: владелец жмёт «offline»/ставит агента на паузу в UI, десктоп пишет в свой каталог, а
/// agentd грузит local-first-дефолты из несуществующего/другого файла → **kill-switch молча неэффективен**
/// (headless продолжит эгресс/работу). Поэтому при смене bundle identifier ОБЯЗАТЕЛЬНО менять и
/// [`NEXUS_BUNDLE_DIR`] (либо задавать `NEXUS_CONFIG_DIR` явно на тот же каталог). `NEXUS_CONFIG_DIR` —
/// штатный способ переопределить локацию (нестандартный config-dir / контейнер / тест), указывая на
/// каталог десктопа.
fn egress_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("NEXUS_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    dirs::config_dir().map(|d| d.join(NEXUS_BUNDLE_DIR))
}

/// CORE-2a tail (AGENT-3e §5): RESTORE персистентного egress kill-switch. Грузит `egress.json` из
/// app-config-dir (зеркало `AppState::apply_egress_state` десктопа) и применяет: `offline` → общий
/// атомик политики; chat/embed/probe → per-feature opt-out. Нет файла/нет config-dir → local-first-
/// дефолты (политика уже построена с offline=false + фичи ON). Логирует применённое (наблюдаемость).
pub(crate) fn apply_persisted_egress(egress_offline: &Arc<AtomicBool>, policy: &Arc<EgressPolicy>) {
    let Some(dir) = egress_config_dir() else {
        tracing::info!(
            "egress.json: OS config-dir не определён — kill-switch local-first (дефолты)"
        );
        return;
    };
    apply_egress_from_dir(&dir, egress_offline, policy);
}

/// Применить `egress.json` из КОНКРЕТНОГО каталога (разделено из [`apply_persisted_egress`] для тестов
/// без зависимости от env/OS config-dir). offline → общий с политикой атомик; chat/embed/probe →
/// per-feature opt-out. Нет файла/битый → local-first-дефолты (`net::persist::load`).
fn apply_egress_from_dir(dir: &Path, egress_offline: &Arc<AtomicBool>, policy: &Arc<EgressPolicy>) {
    let path = dir.join("egress.json");
    let existed = path.exists();
    let st = nexus_core::net::load_egress_state(&path);
    // offline — общий с политикой атомик (политика читает его в check()).
    egress_offline.store(st.offline, std::sync::atomic::Ordering::Relaxed);
    policy.set_feature_enabled(EgressFeature::Chat, st.chat);
    policy.set_feature_enabled(EgressFeature::Embed, st.embed);
    policy.set_feature_enabled(EgressFeature::Probe, st.probe);
    if existed {
        tracing::info!(
            path = %path.display(),
            offline = st.offline,
            chat = st.chat,
            embed = st.embed,
            probe = st.probe,
            "egress.json восстановлен — kill-switch владельца применён (headless)"
        );
    } else {
        tracing::info!(
            path = %path.display(),
            "egress.json отсутствует — kill-switch local-first (дефолты: online, фичи ON)"
        );
    }
}

/// KILL-SWITCH (AGENT-5): RESTORE персистентной паузы агента. Грузит `agent.json` из app-config-dir
/// (ТОТ ЖЕ каталог, что egress.json — зеркало десктопа/[`egress_config_dir`]) и взводит общий атомик,
/// если `paused=true`. Нет файла/нет config-dir → НЕ на паузе (агент работает из коробки). Логирует.
pub(crate) fn apply_persisted_agent_pause(agent_paused: &Arc<AtomicBool>) {
    let Some(dir) = egress_config_dir() else {
        tracing::info!(
            "agent.json: OS config-dir не определён — kill-switch агента local-first (не на паузе)"
        );
        return;
    };
    apply_agent_pause_from_dir(&dir, agent_paused);
}

/// Применить `agent.json` из КОНКРЕТНОГО каталога (разделено для тестов без env/OS config-dir). Нет
/// файла/битый → дефолт (не на паузе). Зеркало [`apply_egress_from_dir`].
fn apply_agent_pause_from_dir(dir: &Path, agent_paused: &Arc<AtomicBool>) {
    let path = dir.join("agent.json");
    let existed = path.exists();
    let st = nexus_core::agent::load_control_state(&path);
    agent_paused.store(st.paused, std::sync::atomic::Ordering::Relaxed);
    if existed {
        tracing::info!(
            path = %path.display(),
            paused = st.paused,
            "agent.json восстановлен — kill-switch агента применён (headless)"
        );
    } else {
        tracing::info!(
            path = %path.display(),
            "agent.json отсутствует — kill-switch агента local-first (не на паузе)"
        );
    }
}

/// KILL-SWITCH (AGENT-5) рантайм-вход (Unix): SIGUSR1 ТОГГЛИТ `agent_paused` (in-memory). Опциональный
/// сигнальный триггер для headless-оператора (UI-кнопка/control-plane — UI-1). На не-Unix — no-op.
pub(crate) fn spawn_pause_signal_toggle(agent_paused: Arc<AtomicBool>) {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
            Ok(mut sig) => {
                tokio::spawn(async move {
                    while sig.recv().await.is_some() {
                        let was =
                            agent_paused.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                        tracing::warn!(
                            paused = !was,
                            "kill-switch агента ТОГГЛНУТ по SIGUSR1 (рантайм)"
                        );
                    }
                });
            }
            Err(e) => tracing::warn!(error = %e, "SIGUSR1-тоггл паузы не подключён"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = agent_paused; // не-Unix: рантайм-сигнала нет (UI-1 даст кросс-платформенный вход)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::net::{EgressDenied, EgressState};
    use tempfile::TempDir;

    /// Свежая политика + общий offline-атомик (как в `run()`).
    fn fresh_policy() -> (Arc<AtomicBool>, Arc<EgressPolicy>) {
        let offline = Arc::new(AtomicBool::new(false));
        let policy = Arc::new(EgressPolicy::new(offline.clone()));
        (offline, policy)
    }

    /// **CORE-2a tail (AGENT-3e §5): persisted offline=ON ЧЕСТИТСЯ agentd.** Сохраняем egress.json с
    /// offline=true, применяем — политика ДЕНАИТ публичный хост (Offline), но LAN/loopback живут.
    #[test]
    fn persisted_offline_is_honored() {
        let dir = TempDir::new().unwrap();
        nexus_core::net::save_egress_state(
            &dir.path().join("egress.json"),
            &EgressState {
                offline: true,
                chat: true,
                embed: true,
                probe: true,
            },
        )
        .unwrap();

        let (offline, policy) = fresh_policy();
        apply_egress_from_dir(dir.path(), &offline, &policy);

        assert!(
            offline.load(std::sync::atomic::Ordering::Relaxed),
            "offline применён"
        );
        // Публичный хост отрезан kill-switch'ем.
        assert_eq!(
            policy.check("api.example.com", EgressFeature::Chat),
            Err(EgressDenied::Offline),
            "offline=ON: публичный Chat-хост денайнут (kill-switch уважён)"
        );
        // LAN/loopback живут даже в офлайне (local-first).
        assert!(
            policy.check("127.0.0.1", EgressFeature::Chat).is_ok(),
            "loopback живёт в офлайне"
        );
    }

    /// Per-feature opt-out из egress.json ЧЕСТИТСЯ: chat=false → Chat-фича выключена даже к loopback.
    #[test]
    fn persisted_feature_optout_is_honored() {
        let dir = TempDir::new().unwrap();
        nexus_core::net::save_egress_state(
            &dir.path().join("egress.json"),
            &EgressState {
                offline: false,
                chat: false,
                embed: true,
                probe: true,
            },
        )
        .unwrap();

        let (offline, policy) = fresh_policy();
        apply_egress_from_dir(dir.path(), &offline, &policy);

        assert!(
            !policy.is_feature_enabled(EgressFeature::Chat),
            "chat opt-out применён"
        );
        assert_eq!(
            policy.check("127.0.0.1", EgressFeature::Chat),
            Err(EgressDenied::FeatureNotEnabled(EgressFeature::Chat)),
            "chat=false: даже loopback Chat выключен"
        );
        assert!(
            policy.check("127.0.0.1", EgressFeature::Embed).is_ok(),
            "embed остался ON"
        );
    }

    /// Нет файла → local-first-дефолты (online, фичи ON) — fail-safe, не валит старт.
    #[test]
    fn missing_egress_json_is_local_first_defaults() {
        let dir = TempDir::new().unwrap(); // пуст, файла нет
        let (offline, policy) = fresh_policy();
        apply_egress_from_dir(dir.path(), &offline, &policy);

        assert!(
            !offline.load(std::sync::atomic::Ordering::Relaxed),
            "online по умолчанию"
        );
        assert!(policy.is_feature_enabled(EgressFeature::Chat));
        assert!(policy.is_feature_enabled(EgressFeature::Embed));
        assert!(policy.is_feature_enabled(EgressFeature::Probe));
    }

    /// `NEXUS_CONFIG_DIR` переопределяет локацию (явный путь приоритетнее OS config-dir).
    /// Env-тест изолирован (один тест трогает env; остальные не зависят от него).
    #[test]
    fn config_dir_env_override() {
        std::env::set_var("NEXUS_CONFIG_DIR", "/tmp/nexus-test-cfg-xyz");
        assert_eq!(
            egress_config_dir(),
            Some(PathBuf::from("/tmp/nexus-test-cfg-xyz"))
        );
        std::env::remove_var("NEXUS_CONFIG_DIR");
    }

    // ── AGENT-5: KILL-SWITCH персист (agent.json restore) ─────────────────────────────────────────

    /// **persisted paused=ON ЧЕСТИТСЯ agentd.** Сохраняем agent.json с paused=true, применяем —
    /// общий атомик kill-switch взведён (хендлер увидит паузу с самого старта).
    #[test]
    fn persisted_agent_pause_is_honored() {
        let dir = TempDir::new().unwrap();
        nexus_core::agent::save_control_state(
            &dir.path().join("agent.json"),
            &nexus_core::agent::AgentControlState { paused: true },
        )
        .unwrap();

        let agent_paused = Arc::new(AtomicBool::new(false));
        apply_agent_pause_from_dir(dir.path(), &agent_paused);
        assert!(
            agent_paused.load(std::sync::atomic::Ordering::Relaxed),
            "persisted paused=true применён (kill-switch агента взведён)"
        );
    }

    /// Нет agent.json (первый запуск) → НЕ на паузе (агент работает из коробки) — fail-safe старта.
    #[test]
    fn missing_agent_json_is_not_paused() {
        let dir = TempDir::new().unwrap(); // пуст
        let agent_paused = Arc::new(AtomicBool::new(false));
        apply_agent_pause_from_dir(dir.path(), &agent_paused);
        assert!(
            !agent_paused.load(std::sync::atomic::Ordering::Relaxed),
            "нет файла → агент НЕ на паузе (работает из коробки)"
        );
    }
}
