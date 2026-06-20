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
}
