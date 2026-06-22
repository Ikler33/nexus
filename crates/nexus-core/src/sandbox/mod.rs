//! OS-песочница прогона агента — Фаза-2 каркас (`docs/specs/agent-sandbox.md`).
//!
//! **SANDBOX-1 (этот срез):** ЧИСТЫЙ рендер `podman run` argv (config → план) + мастер-флаг
//! [`crate::ai::AiConfig::sandbox_enabled`] (default-OFF). БЕЗ рантайма (`podman` не запускается),
//! БЕЗ GuardedProxy (egress — SANDBOX-2), БЕЗ host-actuator (Фаза-3). Рендер отделён от актуации ровно
//! как `nexus-cli::service::docker_plan` (argv-векторы, без шелла) — но ЖИВЁТ в ядре, т.к. будущий
//! `SandboxRunner` (`JobHandler`, SANDBOX-4) вызывает его, а `nexus-cli` зависит от ядра, не наоборот.
//!
//! Хардненинг (спека §3.1): `--network=none` (нет NIC — единственный сетевой путь будет GuardedProxy по
//! AF_UNIX, SANDBOX-2), `--read-only` rootfs + `--tmpfs /tmp`, `--cap-drop=ALL`,
//! `--security-opt no-new-privileges`, `--userns=keep-id` (uid контейнера = host-uid владельца vault),
//! ресурс-кэпы; vault bind **`:ro`**; per-run каталог сокетов — ОТДЕЛЬНЫЙ mount, НЕ под `:ro`-vault
//! (спека §4.4, анти-footgun). Окружение хоста НЕ пробрасывается (нет `--env` — env-scrub fail-closed
//! придёт в SANDBOX-6a; на этом срезе важно лишь, что секреты хоста физически не утекают в argv).

use std::path::{Path, PathBuf};

/// GuardedProxy — единственный сетевой путь песочного прогона (`--network=none` + AF_UNIX-прокси поверх
/// существующего `GuardedClient`). SANDBOX-2.
pub mod proxy;

/// host/act — RPC vault-записи (vault `:ro` в контейнере → записи host-side через `dispatch_action`).
/// SANDBOX-3.
pub mod act;

/// `ProxyToolProvider` — in-sandbox tool-capable провайдер (stream:false поверх GuardedProxy). SANDBOX-4a.
pub mod provider;

/// OUTWARD-форвардер событий: in-sandbox `ProxyEventForwarder` → event.sock → host `EventForwardServer`
/// → реальный host-форвардер (события хода → десктоп). SANDBOX-4b.
pub mod event;

/// Образ песочницы по умолчанию (тот же, что у DEPLOY-3 `nexus deploy docker`).
pub const DEFAULT_SANDBOX_IMAGE: &str = "nexus-agentd:local";
/// Путь vault ВНУТРИ контейнера (`:ro`), = `NEXUS_VAULT` образа.
pub const CONTAINER_VAULT: &str = "/vault";
/// Каталог per-run сокетов ВНУТРИ контейнера (GuardedProxy/control — SANDBOX-2+). НЕ под vault.
pub const CONTAINER_RUN_DIR: &str = "/run/nexus";

/// Ресурс-кэпы контейнера. Консервативные дефолты (спека §12 — владелец может настроить под профиль хоста).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceCaps {
    /// `--pids-limit`.
    pub pids: u32,
    /// `--memory` (напр. `"2g"`).
    pub memory: String,
    /// `--cpus` (напр. `"2"`).
    pub cpus: String,
}

impl Default for ResourceCaps {
    fn default() -> Self {
        Self {
            pids: 512,
            memory: "2g".into(),
            cpus: "2".into(),
        }
    }
}

/// Параметры рендера одного песочного прогона. Конструируется через [`SandboxConfig::for_run`]
/// (валидирует `run_id` + структурно выносит каталог сокетов из-под vault).
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Имя образа `name:tag`.
    pub image: String,
    /// Идентификатор прогона → имя контейнера `nexus-run-<run_id>` + per-run каталог сокетов.
    pub run_id: String,
    /// Абсолютный путь vault на ХОСТЕ (bind `:ro` в `/vault`).
    pub host_vault: PathBuf,
    /// Per-run каталог сокетов на ХОСТЕ (bind в `/run/nexus`). НЕ под vault (спека §4.4).
    pub host_run_dir: PathBuf,
    /// Ресурс-кэпы контейнера.
    pub caps: ResourceCaps,
}

impl SandboxConfig {
    /// Конструктор: валидирует `run_id` (непустой, `[A-Za-z0-9._-]`, не с `-`/`.` — иначе невалидное имя
    /// контейнера podman) и ДЕРИВИТ каталог сокетов из `runtime_base` (напр. `XDG_RUNTIME_DIR`), который
    /// ОБЯЗАН быть вне vault. Структурно гарантирует инвариант §4.4 (сокеты не под `:ro`-vault).
    pub fn for_run(
        image: impl Into<String>,
        run_id: impl Into<String>,
        host_vault: impl Into<PathBuf>,
        runtime_base: &Path,
        caps: ResourceCaps,
    ) -> Result<Self, String> {
        let run_id = run_id.into();
        validate_run_id(&run_id)?;
        let host_vault = host_vault.into();
        // POSIX-join (`/`), НЕ `PathBuf::join`: песочница — Linux-host-only, путь всегда POSIX. `join`
        // вставил бы `\` на Windows → backslash в Linux-`-v`-байнде + падение юнит-теста на Windows-CI
        // (тот же класс, что фикс posix-путей в `nexus-cli deploy remote`).
        let base = runtime_base.to_string_lossy();
        let host_run_dir = PathBuf::from(format!(
            "{}/{}",
            base.trim_end_matches('/'),
            container_name(&run_id)
        ));
        // Анти-footgun §4.4: каталог сокетов НЕ должен лежать внутри vault (vault монтируется :ro →
        // сокет там не забиндить; и смешивать rw-сокеты с :ro-данными нельзя). Лексическая проверка
        // (пути ещё могут не существовать — canonicalize неприменим).
        if host_run_dir.starts_with(&host_vault) {
            return Err(format!(
                "каталог сокетов {} не должен быть внутри vault {} (нужен runtime_base вне vault)",
                host_run_dir.display(),
                host_vault.display()
            ));
        }
        Ok(Self {
            image: image.into(),
            run_id,
            host_vault,
            host_run_dir,
            caps,
        })
    }
}

/// Имя контейнера прогона. Подчиняется формату podman `[a-zA-Z0-9][a-zA-Z0-9_.-]*` при валидном `run_id`.
pub fn container_name(run_id: &str) -> String {
    format!("nexus-run-{run_id}")
}

/// Валидирует `run_id` для встраивания в имя контейнера/путь: непустой, только `[A-Za-z0-9._-]`,
/// начинается с буквы/цифры (podman-формат). `run_id` — ВНУТРЕННИЙ (из `agent_runs`), но валидируем
/// fail-closed на случай смены генератора.
pub fn validate_run_id(run_id: &str) -> Result<(), String> {
    match run_id.chars().next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => {
            return Err(format!(
                "run_id должен начинаться с буквы/цифры: {run_id:?}"
            ))
        }
    }
    if let Some(c) = run_id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
    {
        return Err(format!(
            "run_id содержит недопустимый символ {c:?}: {run_id}"
        ));
    }
    Ok(())
}

/// План песочного прогона: argv `podman run` (исполняется БЕЗ шелла) + имя контейнера.
#[derive(Debug, Clone)]
pub struct SandboxPlan {
    /// Полный argv-вектор (`podman run … <image>`).
    pub argv: Vec<String>,
    /// Имя контейнера (`nexus-run-<run_id>`) — для cancel (`podman kill`) / teardown.
    pub container_name: String,
}

/// Рендер хардненного `podman run` argv. ЧИСТАЯ функция (Tier-1-тестируема). Рантайм НЕ запускает,
/// НЕ передаёт окружение хоста. Egress замкнут `--network=none` (GuardedProxy подключит SANDBOX-2).
pub fn sandbox_run_plan(cfg: &SandboxConfig) -> SandboxPlan {
    let name = container_name(&cfg.run_id);
    let argv = vec![
        "podman".into(),
        "run".into(),
        "--rm".into(), // эфемерный: состояние не переживает прогон
        "--name".into(),
        name.clone(),
        "--network=none".into(), // НЕТ NIC — egress ТОЛЬКО через GuardedProxy (SANDBOX-2)
        "--read-only".into(),    // read-only rootfs
        "--tmpfs".into(),
        "/tmp".into(), // writable scratch (rootfs read-only)
        "--cap-drop=ALL".into(),
        "--security-opt=no-new-privileges".into(), // `=`-форма (как --cap-drop=/--network=), спека §3.1
        "--userns=keep-id".into(), // uid контейнера = host-uid (владелец vault-bind)
        "--pids-limit".into(),
        cfg.caps.pids.to_string(),
        "--memory".into(),
        cfg.caps.memory.clone(),
        "--cpus".into(),
        cfg.caps.cpus.clone(),
        // vault — READ-ONLY (агент читает заметки; записи идут host-side через host/act, SANDBOX-3).
        "-v".into(),
        format!("{}:{CONTAINER_VAULT}:ro", cfg.host_vault.display()),
        // per-run сокеты (rw) — ОТДЕЛЬНЫЙ mount, НЕ под vault (§4.4).
        "-v".into(),
        format!("{}:{CONTAINER_RUN_DIR}", cfg.host_run_dir.display()),
        cfg.image.clone(),
    ];
    SandboxPlan {
        argv,
        container_name: name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SandboxConfig {
        SandboxConfig::for_run(
            DEFAULT_SANDBOX_IMAGE,
            "run123",
            PathBuf::from("/home/u/vault"),
            Path::new("/run/user/1000"),
            ResourceCaps::default(),
        )
        .unwrap()
    }

    #[test]
    fn for_run_derives_socket_dir_outside_vault() {
        let c = cfg();
        assert_eq!(
            c.host_run_dir,
            PathBuf::from("/run/user/1000/nexus-run-run123")
        );
        assert!(!c.host_run_dir.starts_with(&c.host_vault));
    }

    #[test]
    fn for_run_rejects_socket_dir_inside_vault() {
        // runtime_base внутри vault → отказ (анти-footgun §4.4).
        let r = SandboxConfig::for_run(
            DEFAULT_SANDBOX_IMAGE,
            "run123",
            PathBuf::from("/home/u/vault"),
            Path::new("/home/u/vault/.nexus"),
            ResourceCaps::default(),
        );
        assert!(r.is_err());
    }

    #[test]
    fn run_id_validation() {
        assert!(validate_run_id("run123").is_ok());
        assert!(validate_run_id("a1b2-c3.d4_e5").is_ok());
        assert!(validate_run_id("").is_err());
        assert!(validate_run_id("-x").is_err()); // не с '-' (podman-имя)
        assert!(validate_run_id(".x").is_err());
        assert!(validate_run_id("a/b").is_err());
        assert!(validate_run_id("a b").is_err());
        assert!(validate_run_id("a;rm").is_err());
    }

    #[test]
    fn plan_has_all_hardening_flags() {
        let p = sandbox_run_plan(&cfg());
        let a = &p.argv;
        let has = |s: &str| a.iter().any(|x| x == s);
        assert_eq!(a[0], "podman");
        assert_eq!(a[1], "run");
        assert!(has("--rm"));
        assert!(has("--network=none"), "нет NIC — ключевой инвариант egress");
        assert!(has("--read-only"));
        assert!(has("--cap-drop=ALL"));
        assert!(has("--security-opt=no-new-privileges"));
        assert!(has("--userns=keep-id"));
        assert!(has("--pids-limit"));
        assert!(has("--memory"));
        assert!(has("--cpus"));
        // --tmpfs /tmp (пара)
        let tmpfs = a.iter().position(|x| x == "--tmpfs").expect("--tmpfs");
        assert_eq!(a[tmpfs + 1], "/tmp");
        // образ — последним аргументом.
        assert_eq!(a.last().unwrap(), DEFAULT_SANDBOX_IMAGE);
        assert_eq!(p.container_name, "nexus-run-run123");
    }

    #[test]
    fn plan_vault_is_readonly_and_sockets_are_distinct_mount() {
        let p = sandbox_run_plan(&cfg());
        let a = &p.argv;
        // vault bind — РОВНО :ro.
        assert!(
            a.iter().any(|x| x == "/home/u/vault:/vault:ro"),
            "vault должен биндиться :ro: {a:?}"
        );
        // сокеты — ОТДЕЛЬНЫЙ mount в /run/nexus, НЕ под vault, БЕЗ :ro.
        assert!(
            a.iter()
                .any(|x| x == "/run/user/1000/nexus-run-run123:/run/nexus"),
            "сокеты — distinct mount вне vault: {a:?}"
        );
        // никакой сокет-bind не лежит под :ro-vault путём.
        assert!(
            !a.iter()
                .any(|x| x.contains("/vault/") && x.contains("/run/nexus")),
            "сокет НЕ под vault: {a:?}"
        );
        // POSIX-регресс-гард: ни одного бэкслеша (Linux-host пути; `PathBuf::join` на Windows дал бы `\`).
        assert!(
            !a.iter().any(|x| x.contains('\\')),
            "argv должен быть POSIX (без бэкслешей): {a:?}"
        );
    }

    #[test]
    fn plan_passes_no_host_env() {
        // env-scrub fail-closed (SANDBOX-6a) ещё впереди, но УЖЕ сейчас: хост-окружение НЕ
        // пробрасывается в контейнер (нет -e/--env) → секреты хоста физически не утекают в argv.
        let p = sandbox_run_plan(&cfg());
        assert!(
            !p.argv.iter().any(|x| x == "-e" || x == "--env"),
            "хост-env не должен пробрасываться: {:?}",
            p.argv
        );
    }
}
