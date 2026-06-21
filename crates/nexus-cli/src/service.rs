//! Платформенный сервис-план для `nexus-agentd` (PROD-v1 deploy): рендер юнита + пути + команды
//! загрузки/выгрузки. **macOS → launchd** (`~/Library/LaunchAgents/<label>.plist`), **Linux → systemd
//! --user** (`~/.config/systemd/user/<unit>`). Рендер — ЧИСТЫЙ (тестируемый); актуация (запись файла +
//! `launchctl`/`systemctl`) — в [`crate`] под явным `--apply` (safe default — печать плана).

use std::path::PathBuf;

/// Метка launchd-сервиса (= имя plist-файла без расширения).
pub const LAUNCHD_LABEL: &str = "com.nexus.agentd";
/// Имя systemd --user юнита.
pub const SYSTEMD_UNIT: &str = "nexus-agentd.service";

/// Параметры деплоя локального агент-сервиса (всё уже резолвлено в абсолютные пути).
#[derive(Debug, Clone)]
pub struct DeployConfig {
    /// КАНОНИЗИРОВАННЫЙ корень vault (аргумент agentd).
    pub vault: PathBuf,
    /// Путь к бинарю `nexus-agentd`.
    pub agentd_bin: PathBuf,
    /// AF_UNIX-сокет коннектора (env `NEXUS_AGENTD_CONNECT_SOCKET` сервиса).
    pub socket: PathBuf,
    /// Каталог под stdout/stderr сервиса.
    pub log_dir: PathBuf,
}

/// Тип системы инициализации сервисов целевой ОС.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    /// macOS launchd (per-user LaunchAgent).
    Launchd,
    /// Linux systemd (per-user unit).
    Systemd,
    /// Прочее (Windows/неизвестно) — деплой как сервис не поддержан (запускать agentd вручную).
    Unsupported,
}

/// Определяет систему сервисов по целевой ОС сборки.
pub fn detect_kind() -> ServiceKind {
    if cfg!(target_os = "macos") {
        ServiceKind::Launchd
    } else if cfg!(target_os = "linux") {
        ServiceKind::Systemd
    } else {
        ServiceKind::Unsupported
    }
}

/// Полный план установки сервиса: что записать и какие команды выполнить (для `--apply`) / напечатать
/// (dry-run). `*_cmds` — списки argv (программа + аргументы), выполняются по порядку.
#[derive(Debug, Clone)]
pub struct ServicePlan {
    pub kind: ServiceKind,
    /// Человекочитаемая метка/имя юнита.
    pub label: String,
    /// Куда записать юнит.
    pub unit_path: PathBuf,
    /// Содержимое юнита.
    pub unit_content: String,
    /// Команды загрузки/старта (после записи юнита) — для `deploy --apply`. Команды ВЫГРУЗКИ — у
    /// [`undeploy_plan`] (cfg-независимы), сюда не дублируем.
    pub load_cmds: Vec<Vec<String>>,
}

/// Экранирование для XML-`<string>` plist (пути теоретически могут нести `& < >`). `"` в text-контенте
/// XML валиден без экранирования (нужен только в атрибутах) — `<string>`-контент его не требует.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Экранирование для systemd-юнита (двойные кавычки в `ExecStart="..."` / `Environment="..."`): бэкслеш и
/// `"` экранируются бэкслешем (systemd понимает C-style escaping в кавычках). Путь с `"`/пробелом не
/// ломает синтаксис юнита.
fn systemd_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Рендер launchd-plist: запускает `agentd <vault>` с env-сокетом, рестарт (KeepAlive), логи в файлы.
pub fn render_launchd_plist(cfg: &DeployConfig) -> String {
    let bin = xml_escape(&cfg.agentd_bin.display().to_string());
    let vault = xml_escape(&cfg.vault.display().to_string());
    let socket = xml_escape(&cfg.socket.display().to_string());
    let out = xml_escape(&cfg.log_dir.join("agentd.out.log").display().to_string());
    let err = xml_escape(&cfg.log_dir.join("agentd.err.log").display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>{vault}</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>NEXUS_AGENTD_CONNECT_SOCKET</key>
    <string>{socket}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{out}</string>
  <key>StandardErrorPath</key>
  <string>{err}</string>
</dict>
</plist>
"#
    )
}

/// Рендер systemd --user юнита: `ExecStart=agentd vault`, env-сокет, рестарт при сбое. Пути — в кавычках
/// с systemd-экранированием (пробел/`"` в пути не ломают синтаксис).
pub fn render_systemd_unit(cfg: &DeployConfig) -> String {
    let bin = systemd_escape(&cfg.agentd_bin.display().to_string());
    let vault = systemd_escape(&cfg.vault.display().to_string());
    // Environment в КАВЫЧКАХ целиком (`Environment="KEY=value"`) — сохраняет пробелы; значение экранируем.
    let socket = systemd_escape(&cfg.socket.display().to_string());
    format!(
        r#"[Unit]
Description=Nexus agent service (nexus-agentd)
After=network.target

[Service]
Type=simple
ExecStart="{bin}" "{vault}"
Environment="NEXUS_AGENTD_CONNECT_SOCKET={socket}"
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
"#
    )
}

/// CFG-НЕЗАВИСИМЫЕ путь юнита + label + команды load/unload (зависят только от `kind`+`home`). Единый
/// источник для [`plan`] (добавляет рендер контента) и [`undeploy_plan`] (контент не нужен) — нет дрейфа.
#[allow(clippy::type_complexity)]
fn unit_layout(
    kind: ServiceKind,
    home: &std::path::Path,
) -> Result<(String, PathBuf, Vec<Vec<String>>, Vec<Vec<String>>), String> {
    match kind {
        ServiceKind::Launchd => {
            let unit_path = home
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{LAUNCHD_LABEL}.plist"));
            let p = unit_path.display().to_string();
            Ok((
                LAUNCHD_LABEL.to_string(),
                unit_path,
                // Идемпотентность: выгрузить прежний (игнор ошибки на уровне исполнителя) → загрузить.
                vec![
                    vec!["launchctl".into(), "unload".into(), p.clone()],
                    vec!["launchctl".into(), "load".into(), "-w".into(), p.clone()],
                ],
                vec![vec!["launchctl".into(), "unload".into(), "-w".into(), p]],
            ))
        }
        ServiceKind::Systemd => {
            let unit_path = home
                .join(".config")
                .join("systemd")
                .join("user")
                .join(SYSTEMD_UNIT);
            Ok((
                SYSTEMD_UNIT.to_string(),
                unit_path,
                vec![
                    vec!["systemctl".into(), "--user".into(), "daemon-reload".into()],
                    vec![
                        "systemctl".into(),
                        "--user".into(),
                        "enable".into(),
                        "--now".into(),
                        SYSTEMD_UNIT.into(),
                    ],
                ],
                vec![vec![
                    "systemctl".into(),
                    "--user".into(),
                    "disable".into(),
                    "--now".into(),
                    SYSTEMD_UNIT.into(),
                ]],
            ))
        }
        ServiceKind::Unsupported => Err(
            "деплой как сервис не поддержан на этой ОС (только macOS launchd / Linux systemd --user); \
             запускайте nexus-agentd вручную с NEXUS_AGENTD_CONNECT_SOCKET"
                .into(),
        ),
    }
}

/// Строит [`ServicePlan`] для текущей ОС. `home` — домашний каталог (для путей юнита); вынесен параметром
/// ради тестируемости (тест передаёт temp-dir).
pub fn plan(
    cfg: &DeployConfig,
    kind: ServiceKind,
    home: &std::path::Path,
) -> Result<ServicePlan, String> {
    let (label, unit_path, load_cmds, _unload_cmds) = unit_layout(kind, home)?;
    let unit_content = match kind {
        ServiceKind::Launchd => render_launchd_plist(cfg),
        ServiceKind::Systemd => render_systemd_unit(cfg),
        ServiceKind::Unsupported => unreachable!("unit_layout вернул бы Err"),
    };
    Ok(ServicePlan {
        kind,
        label,
        unit_path,
        unit_content,
        load_cmds,
    })
}

/// CFG-независимый план выгрузки (для `undeploy`): путь юнита + команды остановки. Не требует
/// [`DeployConfig`] — undeploy не рендерит контент.
pub fn undeploy_plan(
    kind: ServiceKind,
    home: &std::path::Path,
) -> Result<(String, PathBuf, Vec<Vec<String>>), String> {
    let (label, unit_path, _load, unload_cmds) = unit_layout(kind, home)?;
    Ok((label, unit_path, unload_cmds))
}

// ── Удалённый деплой (DEPLOY-2) ──────────────────────────────────────────────────────────────────
//
// Цель — Linux-хост с `systemd --user` (риг 192.168.0.31, на нём локальный LLM). Бинарь agentd
// доставляется `scp`, юнит — `systemd --user`, запуск — `systemctl --user enable --now`. Рендер плана —
// ЧИСТЫЙ/тестируемый; актуация (ssh/scp) — в [`crate`] под `--apply`. Удалённые пути ОБЯЗАНЫ быть
// «чистыми» абсолютными (валидирует [`crate::validate_remote_path`]) — встраиваются в ssh-команды без
// shell-экранирования.

/// Имя файла бинаря agentd на удалённом хосте (внутри `<home>/.nexus/bin/`).
pub const REMOTE_BIN_NAME: &str = "nexus-agentd";

/// Параметры удалённого деплоя (Linux systemd --user).
#[derive(Debug, Clone)]
pub struct RemoteConfig {
    /// Удалённый пользователь (ssh + `loginctl enable-linger`).
    pub user: String,
    /// Удалённый хост (IP/DNS).
    pub host: String,
    /// Домашний каталог удалённого пользователя (для путей бинаря/юнита). Абсолютный.
    pub remote_home: PathBuf,
    /// Локальный путь к Linux-бинарю agentd (источник `scp`).
    pub local_binary: PathBuf,
    /// Удалённый корень vault (аргумент agentd). Абсолютный.
    pub remote_vault: PathBuf,
    /// Удалённый AF_UNIX-сокет коннектора. Абсолютный.
    pub remote_socket: PathBuf,
}

/// Шаг удалённого деплоя (исполняется по порядку под `--apply`).
#[derive(Debug, Clone)]
pub enum RemoteStep {
    /// Shell-команда на удалённом хосте (`ssh <target> <cmd>`). `best_effort` — сбой НЕ прерывает план
    /// (напр. `loginctl enable-linger` требует polkit/root и может не пройти — это не фатально для запуска).
    Run { cmd: String, best_effort: bool },
    /// `scp <local_binary> <target>:<remote_bin>`.
    PutBinary,
    /// Записать юнит во временный файл и `scp` его в `<target>:<remote_unit_path>`.
    PutUnit,
}

/// Полный план удалённого деплоя: контент юнита + упорядоченные ssh/scp-шаги.
#[derive(Debug, Clone)]
pub struct RemotePlan {
    /// `user@host` (аргумент ssh/scp).
    pub target: String,
    /// Абсолютный путь бинаря на удалённом хосте.
    pub remote_bin: PathBuf,
    /// Абсолютный путь юнита на удалённом хосте.
    pub remote_unit_path: PathBuf,
    /// Содержимое systemd-юнита (ссылается на УДАЛЁННЫЕ абсолютные пути).
    pub unit_content: String,
    /// Упорядоченные шаги.
    pub steps: Vec<RemoteStep>,
}

/// Домашний каталог удалённого пользователя по соглашению: `root → /root`, иначе `/home/<user>`.
/// Переопределяется `--remote-home` на уровне CLI.
pub fn default_remote_home(user: &str) -> PathBuf {
    if user == "root" {
        PathBuf::from("/root")
    } else {
        PathBuf::from(format!("/home/{user}"))
    }
}

/// POSIX-join для УДАЛЁННЫХ путей: всегда `/`-разделитель, НЕЗАВИСИМО от ОС хоста, где запущен CLI.
/// `PathBuf::join` вставил бы `\` на Windows → сломанный Linux-юнит/`mkdir` при деплое С Windows на риг
/// (и падение юнит-тестов на Windows-CI). `tail` — POSIX-хвост (`".nexus/bin"`).
pub fn posix_join(base: &std::path::Path, tail: &str) -> PathBuf {
    let b = base.to_string_lossy();
    let b = b.trim_end_matches('/');
    PathBuf::from(format!("{b}/{tail}"))
}

/// `systemctl --user` по ssh идёт В НЕ-логин-сессии (нет `XDG_RUNTIME_DIR`) → задаём явно перед командой.
const XDG_PREFIX: &str = "export XDG_RUNTIME_DIR=/run/user/$(id -u);";

/// Строит [`RemotePlan`]. ЧИСТАЯ функция (тестируема). Удалённые пути в `cfg` ДОЛЖНЫ быть провалидированы
/// вызывающим как «чистые» — здесь они встраиваются в shell-команды без экранирования.
pub fn remote_plan(cfg: &RemoteConfig) -> RemotePlan {
    let target = format!("{}@{}", cfg.user, cfg.host);
    // Удалённые пути — ВСЕГДА POSIX (`/`), даже если CLI запущен на Windows (см. `posix_join`).
    let bin_dir = posix_join(&cfg.remote_home, ".nexus/bin");
    let remote_bin = posix_join(&bin_dir, REMOTE_BIN_NAME);
    let unit_dir = posix_join(&cfg.remote_home, ".config/systemd/user");
    let remote_unit_path = posix_join(&unit_dir, SYSTEMD_UNIT);
    let log_dir = posix_join(&cfg.remote_vault, ".nexus/logs");

    let unit_content = render_systemd_unit(&DeployConfig {
        vault: cfg.remote_vault.clone(),
        agentd_bin: remote_bin.clone(),
        socket: cfg.remote_socket.clone(),
        log_dir: log_dir.clone(),
    });

    let d = |p: &std::path::Path| p.display().to_string();
    let mkdir = format!(
        "mkdir -p {} {} {} {}",
        d(&bin_dir),
        d(&unit_dir),
        d(&log_dir),
        d(&cfg.remote_vault)
    );
    let chmod = format!("chmod +x {}", d(&remote_bin));
    let linger = format!("loginctl enable-linger {}", cfg.user);
    let reload = format!("{XDG_PREFIX} systemctl --user daemon-reload");
    let enable = format!("{XDG_PREFIX} systemctl --user enable --now {SYSTEMD_UNIT}");

    let steps = vec![
        RemoteStep::Run {
            cmd: mkdir,
            best_effort: false,
        },
        RemoteStep::PutBinary,
        RemoteStep::Run {
            cmd: chmod,
            best_effort: false,
        },
        RemoteStep::PutUnit,
        RemoteStep::Run {
            cmd: linger,
            best_effort: true,
        },
        RemoteStep::Run {
            cmd: reload,
            best_effort: false,
        },
        RemoteStep::Run {
            cmd: enable,
            best_effort: false,
        },
    ];

    RemotePlan {
        target,
        remote_bin,
        remote_unit_path,
        unit_content,
        steps,
    }
}

/// План СНЯТИЯ удалённого сервиса (симметрия `remote_plan`): остановить+отключить юнит, удалить файл
/// юнита, перечитать systemd. Возвращает `(target, remote_unit_path, steps)`. Все шаги best-effort
/// (снятие отсутствующего сервиса — норма, как у локального `undeploy`). НЕ трогает бинарь/vault —
/// убираем только сервис, не пользовательские данные (паритет с локальным `undeploy`). Нужны только
/// `user`/`host`/`remote_home` (путь юнита) — отдельный лёгкий вход, без `RemoteConfig` (бинарь/vault
/// тут не при чём).
pub fn remote_undeploy_plan(
    user: &str,
    host: &str,
    remote_home: &std::path::Path,
) -> (String, PathBuf, Vec<RemoteStep>) {
    let target = format!("{user}@{host}");
    let unit_dir = posix_join(remote_home, ".config/systemd/user");
    let remote_unit_path = posix_join(&unit_dir, SYSTEMD_UNIT);
    let disable = format!("{XDG_PREFIX} systemctl --user disable --now {SYSTEMD_UNIT}");
    let rm = format!("rm -f {}", remote_unit_path.display());
    let reload = format!("{XDG_PREFIX} systemctl --user daemon-reload");
    let steps = vec![
        RemoteStep::Run {
            cmd: disable,
            best_effort: true,
        },
        RemoteStep::Run {
            cmd: rm,
            best_effort: true,
        },
        RemoteStep::Run {
            cmd: reload,
            best_effort: true,
        },
    ];
    (target, remote_unit_path, steps)
}

// ── Контейнер-деплой (DEPLOY-3) ───────────────────────────────────────────────────────────────────
//
// Запуск agentd в Docker-контейнере (образ — `Dockerfile` в корне репо; см. также Фаза-2 Podman).
// Чистый рендер плана (`docker build`/`docker run` как argv-векторы — БЕЗ шелла) отделён от актуации
// под `--apply`. Коннектор (AF_UNIX) пробрасывается через bind-mount vault → сокет на хосте
// `<vault>/.nexus/agentd.sock` (работает на Linux-хосте; macOS Docker Desktop сокет не пробрасывает).

/// Путь монтирования vault ВНУТРИ контейнера (= `NEXUS_VAULT` образа).
pub const CONTAINER_VAULT: &str = "/vault";
/// Путь AF_UNIX-сокета коннектора ВНУТРИ контейнера (на томе vault → виден на хосте через bind-mount).
pub const CONTAINER_SOCKET: &str = "/vault/.nexus/agentd.sock";

/// Параметры контейнер-деплоя agentd.
#[derive(Debug, Clone)]
pub struct DockerConfig {
    /// Имя образа `name:tag`.
    pub image: String,
    /// Имя контейнера.
    pub container_name: String,
    /// Абсолютный путь vault на ХОСТЕ (bind-mount в `/vault`).
    pub host_vault: PathBuf,
    /// `Some(ctx)` → включить шаг `docker build -t <image> <ctx>` перед запуском.
    pub build_context: Option<PathBuf>,
    /// `Some("uid:gid")` → `docker run --user …`: контейнер бежит под этим uid (по умолчанию образ —
    /// uid 10001). Нужно, чтобы процесс мог писать bind-mount vault и создать сокет, доступный хосту:
    /// `--user $(id -u):$(id -g)` приравнивает контейнер-пользователя к владельцу хост-vault.
    pub run_user: Option<String>,
}

/// План контейнер-деплоя: упорядоченные `docker`-команды (argv) + где хост-коннектор найдёт сокет.
#[derive(Debug, Clone)]
pub struct DockerPlan {
    /// Команды (`docker build`? → `docker run`), argv-векторы, выполняются по порядку.
    pub steps: Vec<Vec<String>>,
    /// Путь сокета на ХОСТЕ (для `nexus status --socket …`).
    pub host_socket: PathBuf,
}

/// Строит [`DockerPlan`]. ЧИСТАЯ функция (тестируема). Образ-внутренние пути — POSIX-константы.
pub fn docker_plan(cfg: &DockerConfig) -> DockerPlan {
    let mut steps: Vec<Vec<String>> = Vec::new();
    if let Some(ctx) = &cfg.build_context {
        steps.push(vec![
            "docker".into(),
            "build".into(),
            "-t".into(),
            cfg.image.clone(),
            ctx.display().to_string(),
        ]);
    }
    let mut run: Vec<String> = vec![
        "docker".into(),
        "run".into(),
        "-d".into(),
        "--name".into(),
        cfg.container_name.clone(),
        "--restart".into(),
        "unless-stopped".into(),
    ];
    if let Some(u) = &cfg.run_user {
        run.push("--user".into());
        run.push(u.clone());
    }
    run.push("-v".into());
    run.push(format!("{}:{CONTAINER_VAULT}", cfg.host_vault.display()));
    run.push("-e".into());
    run.push(format!("NEXUS_AGENTD_CONNECT_SOCKET={CONTAINER_SOCKET}"));
    run.push(cfg.image.clone());
    steps.push(run);
    // Хост-сокет — нативный путь хоста (vault на хосте + .nexus/agentd.sock), зеркалит CONTAINER_SOCKET.
    let host_socket = cfg.host_vault.join(".nexus").join("agentd.sock");
    DockerPlan { steps, host_socket }
}

/// CFG-независимый план остановки контейнера (для `undeploy docker`): `stop` + `rm` по имени.
pub fn docker_undeploy_plan(container_name: &str) -> Vec<Vec<String>> {
    vec![
        vec!["docker".into(), "stop".into(), container_name.into()],
        vec!["docker".into(), "rm".into(), container_name.into()],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> DeployConfig {
        DeployConfig {
            vault: PathBuf::from("/home/u/vault"),
            agentd_bin: PathBuf::from("/usr/local/bin/nexus-agentd"),
            socket: PathBuf::from("/home/u/vault/.nexus/agentd.sock"),
            log_dir: PathBuf::from("/home/u/vault/.nexus/logs"),
        }
    }

    #[test]
    fn launchd_plist_has_args_env_and_socket() {
        let p = render_launchd_plist(&cfg());
        assert!(p.contains("<string>com.nexus.agentd</string>"));
        assert!(p.contains("<string>/usr/local/bin/nexus-agentd</string>"));
        assert!(p.contains("<string>/home/u/vault</string>"));
        assert!(p.contains("NEXUS_AGENTD_CONNECT_SOCKET"));
        assert!(p.contains("/home/u/vault/.nexus/agentd.sock"));
        assert!(p.contains("<key>RunAtLoad</key>") && p.contains("<key>KeepAlive</key>"));
    }

    #[test]
    fn systemd_unit_quotes_paths_and_sets_env() {
        let u = render_systemd_unit(&cfg());
        assert!(u.contains(r#"ExecStart="/usr/local/bin/nexus-agentd" "/home/u/vault""#));
        assert!(u.contains(
            r#"Environment="NEXUS_AGENTD_CONNECT_SOCKET=/home/u/vault/.nexus/agentd.sock""#
        ));
        assert!(u.contains("Restart=on-failure"));
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn systemd_escapes_quotes_in_paths() {
        let c = DeployConfig {
            vault: PathBuf::from(r#"/home/u/va"lt"#),
            agentd_bin: PathBuf::from("/usr/bin/agentd"),
            socket: PathBuf::from(r#"/tmp/s"ock"#),
            log_dir: PathBuf::from("/tmp"),
        };
        let u = render_systemd_unit(&c);
        // `"` внутри пути экранирован бэкслешем → кавычки ExecStart/Environment остаются сбалансированы.
        assert!(
            u.contains(r#""/home/u/va\"lt""#),
            "vault quote escaped: {u}"
        );
        assert!(u.contains(r#"s\"ock""#), "socket quote escaped: {u}");
    }

    #[test]
    fn plan_launchd_paths_and_cmds() {
        let home = std::path::Path::new("/home/u");
        let pl = plan(&cfg(), ServiceKind::Launchd, home).unwrap();
        assert_eq!(
            pl.unit_path,
            PathBuf::from("/home/u/Library/LaunchAgents/com.nexus.agentd.plist")
        );
        assert!(pl.load_cmds.iter().any(|c| c.contains(&"load".to_string())));
        // Команды выгрузки — у undeploy_plan (cfg-независимы).
        let (_l, up, unload) = undeploy_plan(ServiceKind::Launchd, home).unwrap();
        assert_eq!(up, pl.unit_path);
        assert!(unload[0].contains(&"unload".to_string()));
    }

    #[test]
    fn plan_systemd_paths_and_cmds() {
        let home = std::path::Path::new("/home/u");
        let pl = plan(&cfg(), ServiceKind::Systemd, home).unwrap();
        assert_eq!(
            pl.unit_path,
            PathBuf::from("/home/u/.config/systemd/user/nexus-agentd.service")
        );
        assert!(pl
            .load_cmds
            .iter()
            .any(|c| c.contains(&"enable".to_string())));
    }

    #[test]
    fn plan_unsupported_errors() {
        let home = std::path::Path::new("/home/u");
        assert!(plan(&cfg(), ServiceKind::Unsupported, home).is_err());
    }

    #[test]
    fn xml_escape_handles_specials() {
        assert_eq!(xml_escape("a&b<c>d"), "a&amp;b&lt;c&gt;d");
    }

    // ── Удалённый деплой ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn default_remote_home_root_vs_user() {
        assert_eq!(default_remote_home("root"), PathBuf::from("/root"));
        assert_eq!(default_remote_home("artan"), PathBuf::from("/home/artan"));
    }

    fn rcfg() -> RemoteConfig {
        RemoteConfig {
            user: "artan".into(),
            host: "192.168.0.31".into(),
            remote_home: PathBuf::from("/home/artan"),
            local_binary: PathBuf::from("/local/nexus-agentd"),
            remote_vault: PathBuf::from("/home/artan/.nexus/vault"),
            remote_socket: PathBuf::from("/home/artan/.nexus/vault/.nexus/agentd.sock"),
        }
    }

    #[test]
    fn remote_plan_unit_points_at_remote_paths() {
        let p = remote_plan(&rcfg());
        assert_eq!(p.target, "artan@192.168.0.31");
        assert_eq!(
            p.remote_bin,
            PathBuf::from("/home/artan/.nexus/bin/nexus-agentd")
        );
        assert_eq!(
            p.remote_unit_path,
            PathBuf::from("/home/artan/.config/systemd/user/nexus-agentd.service")
        );
        // Юнит ссылается на УДАЛЁННЫЕ абсолютные пути (не на локальные).
        assert!(p.unit_content.contains(
            r#"ExecStart="/home/artan/.nexus/bin/nexus-agentd" "/home/artan/.nexus/vault""#
        ));
        assert!(p
            .unit_content
            .contains("/home/artan/.nexus/vault/.nexus/agentd.sock"));
        // Удалённый юнит — POSIX: НИ ОДНОГО бэкслеша (регресс-гард: `PathBuf::join` на Windows вставлял
        // `\` → сломанный Linux-юнит при деплое С Windows + падение этого теста на Windows-CI).
        assert!(
            !p.unit_content.contains('\\'),
            "unit must be POSIX (no backslash): {}",
            p.unit_content
        );
        assert!(!p.remote_bin.to_string_lossy().contains('\\'));
        assert!(!p.remote_unit_path.to_string_lossy().contains('\\'));
    }

    #[test]
    fn remote_plan_steps_ordered_and_complete() {
        let p = remote_plan(&rcfg());
        // mkdir → scp-binary → chmod → scp-unit → linger(best-effort) → daemon-reload → enable.
        assert!(
            matches!(&p.steps[0], RemoteStep::Run { cmd, best_effort: false } if cmd.starts_with("mkdir -p"))
        );
        assert!(matches!(p.steps[1], RemoteStep::PutBinary));
        assert!(
            matches!(&p.steps[2], RemoteStep::Run { cmd, best_effort: false } if cmd.starts_with("chmod +x"))
        );
        assert!(matches!(p.steps[3], RemoteStep::PutUnit));
        assert!(
            matches!(&p.steps[4], RemoteStep::Run { cmd, best_effort: true } if cmd.contains("enable-linger artan"))
        );
        assert!(
            matches!(&p.steps[5], RemoteStep::Run { cmd, .. } if cmd.contains("systemctl --user daemon-reload"))
        );
        assert!(
            matches!(&p.steps[6], RemoteStep::Run { cmd, .. } if cmd.contains("enable --now nexus-agentd.service"))
        );
    }

    #[test]
    fn remote_plan_mkdir_covers_all_dirs_and_systemctl_sets_xdg() {
        let p = remote_plan(&rcfg());
        let RemoteStep::Run { cmd: mkdir, .. } = &p.steps[0] else {
            panic!("step0 != Run")
        };
        assert!(mkdir.contains("/home/artan/.nexus/bin"));
        assert!(mkdir.contains("/home/artan/.config/systemd/user"));
        assert!(mkdir.contains("/home/artan/.nexus/vault/.nexus/logs"));
        assert!(mkdir.contains("/home/artan/.nexus/vault"));
        // systemctl-шаги несут XDG_RUNTIME_DIR (ssh без логин-сессии).
        let RemoteStep::Run { cmd: reload, .. } = &p.steps[5] else {
            panic!("step5 != Run")
        };
        assert!(reload.contains("XDG_RUNTIME_DIR"));
    }

    #[test]
    fn remote_undeploy_disables_removes_reloads_best_effort() {
        let (target, unit, steps) =
            remote_undeploy_plan("artan", "192.168.0.31", std::path::Path::new("/home/artan"));
        assert_eq!(target, "artan@192.168.0.31");
        assert_eq!(
            unit,
            PathBuf::from("/home/artan/.config/systemd/user/nexus-agentd.service")
        );
        assert_eq!(steps.len(), 3);
        // disable --now → rm unit → daemon-reload; ВСЕ best-effort (снятие отсутствующего — норма).
        assert!(
            matches!(&steps[0], RemoteStep::Run { cmd, best_effort: true } if cmd.contains("disable --now nexus-agentd.service"))
        );
        assert!(
            matches!(&steps[1], RemoteStep::Run { cmd, best_effort: true } if cmd.starts_with("rm -f") && cmd.contains("nexus-agentd.service"))
        );
        assert!(
            matches!(&steps[2], RemoteStep::Run { cmd, best_effort: true } if cmd.contains("daemon-reload"))
        );
        // НЕ трогаем бинарь/vault.
        assert!(!steps
            .iter()
            .any(|s| matches!(s, RemoteStep::Run { cmd, .. } if cmd.contains("/.nexus/bin") || cmd.contains("rm -rf"))));
    }

    // ── Контейнер-деплой ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn docker_plan_run_only_without_build() {
        let p = docker_plan(&DockerConfig {
            image: "nexus-agentd:local".into(),
            container_name: "nexus-agentd".into(),
            host_vault: PathBuf::from("/srv/vault"),
            build_context: None,
            run_user: None,
        });
        assert_eq!(p.steps.len(), 1, "без build_context — только docker run");
        assert!(
            !p.steps[0].contains(&"--user".to_string()),
            "без run_user — нет --user"
        );
        let run = &p.steps[0];
        assert_eq!(run[0], "docker");
        assert_eq!(run[1], "run");
        assert!(run.contains(&"-d".to_string()));
        assert!(run.contains(&"/srv/vault:/vault".to_string()));
        assert!(run.contains(&"NEXUS_AGENTD_CONNECT_SOCKET=/vault/.nexus/agentd.sock".to_string()));
        assert!(run.contains(&"unless-stopped".to_string()));
        assert_eq!(
            run.last().unwrap(),
            "nexus-agentd:local",
            "образ — последним"
        );
        assert_eq!(
            p.host_socket,
            PathBuf::from("/srv/vault/.nexus/agentd.sock")
        );
    }

    #[test]
    fn docker_plan_build_step_precedes_run() {
        let p = docker_plan(&DockerConfig {
            image: "img:t".into(),
            container_name: "c".into(),
            host_vault: PathBuf::from("/v"),
            build_context: Some(PathBuf::from("/repo")),
            run_user: None,
        });
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[0][..4], ["docker", "build", "-t", "img:t"]);
        assert_eq!(p.steps[0][4], "/repo");
        assert_eq!(p.steps[1][1], "run");
    }

    #[test]
    fn docker_plan_run_user_inserts_user_flag() {
        let p = docker_plan(&DockerConfig {
            image: "img:t".into(),
            container_name: "c".into(),
            host_vault: PathBuf::from("/v"),
            build_context: None,
            run_user: Some("1000:1000".into()),
        });
        let run = &p.steps[0];
        let i = run.iter().position(|a| a == "--user").expect("есть --user");
        assert_eq!(run[i + 1], "1000:1000");
        // образ всё ещё последним аргументом.
        assert_eq!(run.last().unwrap(), "img:t");
    }

    #[test]
    fn docker_undeploy_is_stop_then_rm() {
        let u = docker_undeploy_plan("nexus-agentd");
        assert_eq!(u[0], vec!["docker", "stop", "nexus-agentd"]);
        assert_eq!(u[1], vec!["docker", "rm", "nexus-agentd"]);
    }
}
