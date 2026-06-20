//! `nexus` — CLI деплоя/управления агент-сервисом `nexus-agentd` (PROD-v1, item 4).
//!
//! Команды:
//! - `nexus deploy local [--vault P] [--socket P] [--agentd P] [--apply]` — bootstrap `.nexus` + рендер
//!   сервис-юнита (launchd/systemd --user), который запускает `nexus-agentd <vault>` с
//!   `NEXUS_AGENTD_CONNECT_SOCKET`. **Safe default — печать ПЛАНА**; реальная установка только под `--apply`.
//! - `nexus status [--socket P] [--vault P]` — проба коннектора: подключиться к AF_UNIX-сокету и сделать
//!   `initialize` → доступность + версия протокола.
//! - `nexus undeploy [--apply]` — остановить + удалить сервис-юнит (план / `--apply`).
//!
//! Минимум зависимостей (без clap — ручной разбор, как у `nexus-agentd`); сетевого egress нет (только
//! локальный AF_UNIX для `status`).

mod service;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use service::{detect_kind, DeployConfig};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().map(String::as_str).collect();
    match rest.as_slice() {
        [] | ["help"] | ["--help"] | ["-h"] => {
            print_help();
            ExitCode::SUCCESS
        }
        ["deploy", "local", flags @ ..] => run(cmd_deploy_local(flags)),
        ["status", flags @ ..] => run(cmd_status(flags)),
        ["undeploy", flags @ ..] => run(cmd_undeploy(flags)),
        other => {
            eprintln!("nexus: неизвестная команда: {}\n", other.join(" "));
            print_help();
            ExitCode::FAILURE
        }
    }
}

/// Унификация: печатает ошибку и маппит в код возврата.
fn run(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nexus: ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    eprintln!(
        "nexus — управление агент-сервисом nexus-agentd\n\n\
         КОМАНДЫ:\n  \
         deploy local [--vault P] [--socket P] [--agentd P] [--apply]\n      \
         Развернуть agentd локальным сервисом (launchd/systemd). Без --apply — печать плана.\n  \
         status [--socket P] [--vault P]\n      Проверить доступность агента (initialize по AF_UNIX).\n  \
         undeploy [--apply]            Остановить и удалить сервис.\n\n\
         Сокет по умолчанию: <vault>/.nexus/agentd.sock"
    );
}

// ── Разбор флагов ───────────────────────────────────────────────────────────────────────────────────

/// Достаёт `--key value` из плоского списка флагов. `None` — нет ключа ИЛИ за ним идёт другой флаг
/// (`--vault --apply` → не трактуем `--apply` как значение пути). Пути с ведущим `-` не поддерживаем.
fn flag<'a>(flags: &[&'a str], key: &str) -> Option<&'a str> {
    flags
        .iter()
        .position(|f| *f == key)
        .and_then(|i| flags.get(i + 1).copied())
        .filter(|v| !v.starts_with('-'))
}

fn has_flag(flags: &[&str], key: &str) -> bool {
    flags.contains(&key)
}

/// Отвергает пути с управляющими символами (перевод строки/NUL) — они ломают синтаксис plist/systemd-юнита
/// (и не бывают в легитимных путях). Защита перед встраиванием пути в юнит-файл.
fn validate_path_chars(p: &Path, what: &str) -> Result<(), String> {
    let s = p.to_string_lossy();
    if s.contains('\n') || s.contains('\r') || s.contains('\0') {
        return Err(format!(
            "{what} содержит недопустимый символ (перевод строки/NUL): {}",
            p.display()
        ));
    }
    Ok(())
}

// ── Резолюция путей ───────────────────────────────────────────────────────────────────────────────

fn resolve_vault(flags: &[&str]) -> Result<PathBuf, String> {
    let raw = flag(flags, "--vault")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let canon = raw
        .canonicalize()
        .map_err(|e| format!("vault {}: {e}", raw.display()))?;
    if !canon.is_dir() {
        return Err(format!("vault {} — не каталог", canon.display()));
    }
    Ok(canon)
}

/// Путь к бинарю agentd: `--agentd` → сосед текущего exe → `nexus-agentd` из PATH.
fn resolve_agentd(flags: &[&str]) -> PathBuf {
    if let Some(p) = flag(flags, "--agentd") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("nexus-agentd");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("nexus-agentd") // PATH-резолюция системой инициализации
}

/// Сокет коннектора: `--socket` → `<vault>/.nexus/agentd.sock` (дискаверится приложением по vault).
/// Требует АБСОЛЮТНЫЙ путь (юниту нужен абсолютный; relative бессмыслен под launchd/systemd) + без
/// управляющих символов. Дефолт абсолютен (vault канонизирован).
fn resolve_socket(flags: &[&str], vault: &Path) -> Result<PathBuf, String> {
    let raw = flag(flags, "--socket")
        .map(PathBuf::from)
        .unwrap_or_else(|| vault.join(".nexus").join("agentd.sock"));
    if raw.is_relative() {
        return Err(format!(
            "--socket должен быть абсолютным путём: {}",
            raw.display()
        ));
    }
    validate_path_chars(&raw, "socket")?;
    Ok(raw)
}

fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "не удалось определить домашний каталог".to_string())
}

// ── deploy local ──────────────────────────────────────────────────────────────────────────────────

fn cmd_deploy_local(flags: &[&str]) -> Result<(), String> {
    let vault = resolve_vault(flags)?;
    let socket = resolve_socket(flags, &vault)?;
    let agentd_bin = resolve_agentd(flags);
    let log_dir = vault.join(".nexus").join("logs");
    let apply = has_flag(flags, "--apply");

    // Валидация перед встраиванием путей в юнит: без управляющих символов; agentd — АБСОЛЮТНЫЙ (launchd/
    // systemd НЕ резолвят relative/PATH в ExecStart → relative «nexus-agentd» дал бы нерабочий сервис).
    validate_path_chars(&vault, "vault")?;
    validate_path_chars(&agentd_bin, "agentd")?;
    if !agentd_bin.is_absolute() {
        return Err(format!(
            "путь agentd должен быть АБСОЛЮТНЫМ для сервиса — укажите --agentd /abs/path/nexus-agentd \
             (найдено: {})",
            agentd_bin.display()
        ));
    }

    let cfg = DeployConfig {
        vault: vault.clone(),
        agentd_bin,
        socket: socket.clone(),
        log_dir: log_dir.clone(),
    };
    let kind = detect_kind();
    let plan = service::plan(&cfg, kind, &home_dir()?)?;

    println!("=== nexus deploy local ({:?}) ===", plan.kind);
    println!("service:  {}", plan.label);
    println!("vault:    {}", cfg.vault.display());
    println!("agentd:   {}", cfg.agentd_bin.display());
    println!("socket:   {}", cfg.socket.display());
    println!("unit:     {}", plan.unit_path.display());
    println!("\n--- содержимое юнита ---\n{}", plan.unit_content);
    println!("--- команды загрузки ---");
    for c in &plan.load_cmds {
        println!("  {}", c.join(" "));
    }

    let bin_missing = !cfg.agentd_bin.is_file();
    if bin_missing {
        eprintln!(
            "\n⚠ бинарь agentd не найден по {} — соберите `cargo build -p nexus-agentd` или укажите --agentd",
            cfg.agentd_bin.display()
        );
    }
    if let Some(w) = macos_tcc_warning(&cfg) {
        eprintln!("\n{w}");
    }

    if !apply {
        println!("\n(dry-run — план НЕ применён; повторите с --apply для установки сервиса)");
        return Ok(());
    }

    // --apply: НЕ ставим заведомо нерабочий сервис (бинаря нет → ExecStart упадёт).
    if bin_missing {
        return Err(format!(
            "бинарь agentd не найден по {} — соберите/укажите --agentd ПЕРЕД --apply",
            cfg.agentd_bin.display()
        ));
    }
    // bootstrap каталогов (.nexus/logs + родитель сокета [для кастомного --socket вне .nexus]) + запись юнита.
    std::fs::create_dir_all(&log_dir)
        .map_err(|e| format!("создание {}: {e}", log_dir.display()))?;
    if let Some(sp) = cfg.socket.parent() {
        std::fs::create_dir_all(sp).map_err(|e| format!("создание {}: {e}", sp.display()))?;
    }
    if let Some(parent) = plan.unit_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("создание {}: {e}", parent.display()))?;
    }
    std::fs::write(&plan.unit_path, &plan.unit_content)
        .map_err(|e| format!("запись юнита {}: {e}", plan.unit_path.display()))?;
    println!("\n✓ юнит записан: {}", plan.unit_path.display());
    run_cmds(&plan.load_cmds);
    println!(
        "\n✓ деплой применён. Проверка: `nexus status --vault {}`",
        cfg.vault.display()
    );
    Ok(())
}

/// macOS TCC-предупреждение: launchd-агент БЕЗ Full Disk Access не может читать/писать в
/// privacy-защищённые каталоги (`~/Documents`, `~/Desktop`, `~/Downloads`, `/tmp`). Если vault или бинарь
/// agentd там — сервис стартует, но не сможет создать сокет/логи (тихий сбой). Возвращает текст совета,
/// если путь под риском (только на macOS). Проверено эмпирически: vault в `~/Documents`/`/tmp` → сокет не
/// биндится под launchd; перенос в обычный home-каталог (напр. `~/.nexus`) лечит.
fn macos_tcc_warning(cfg: &DeployConfig) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let risky = |p: &Path| {
        let s = p.to_string_lossy();
        s.contains("/Documents/")
            || s.contains("/Desktop/")
            || s.contains("/Downloads/")
            || s.starts_with("/tmp/")
            || s.starts_with("/private/tmp/")
    };
    if risky(&cfg.vault) || risky(&cfg.agentd_bin) {
        Some(
            "⚠ macOS TCC: vault или бинарь agentd в privacy-защищённом каталоге (Documents/Desktop/\
             Downloads//tmp). launchd-агент БЕЗ Full Disk Access там не создаст сокет/логи (тихий сбой). \
             Перенесите vault/бинарь в обычный каталог (напр. ~/.nexus, ~/bin) ИЛИ выдайте Full Disk Access."
                .to_string(),
        )
    } else {
        None
    }
}

// ── undeploy ──────────────────────────────────────────────────────────────────────────────────────

fn cmd_undeploy(flags: &[&str]) -> Result<(), String> {
    let kind = detect_kind();
    // CFG-независимый план выгрузки (путь юнита + команды) — без плейсхолдер-cfg.
    let (label, unit_path, unload_cmds) = service::undeploy_plan(kind, &home_dir()?)?;
    let apply = has_flag(flags, "--apply");

    println!("=== nexus undeploy ({kind:?}) ===");
    println!("service: {label}");
    println!("unit: {}", unit_path.display());
    println!("--- команды выгрузки ---");
    for c in &unload_cmds {
        println!("  {}", c.join(" "));
    }
    if !apply {
        println!("\n(dry-run — повторите с --apply)");
        return Ok(());
    }
    run_cmds(&unload_cmds);
    if unit_path.is_file() {
        std::fs::remove_file(&unit_path)
            .map_err(|e| format!("удаление юнита {}: {e}", unit_path.display()))?;
        println!("✓ юнит удалён: {}", unit_path.display());
    }
    println!("✓ undeploy применён");
    Ok(())
}

/// Выполняет список argv-команд best-effort (печатает исход каждой; не прерывается на сбое — напр.
/// launchd `unload` несуществующего сервиса при первом деплое — нормально).
fn run_cmds(cmds: &[Vec<String>]) {
    for c in cmds {
        let Some((prog, rest)) = c.split_first() else {
            continue;
        };
        match std::process::Command::new(prog).args(rest).status() {
            Ok(st) if st.success() => println!("  ✓ {}", c.join(" ")),
            Ok(st) => println!("  ⚠ {} → код {}", c.join(" "), st.code().unwrap_or(-1)),
            Err(e) => println!("  ⚠ {} → {e}", c.join(" ")),
        }
    }
}

// ── status ────────────────────────────────────────────────────────────────────────────────────────

#[cfg(unix)]
fn cmd_status(flags: &[&str]) -> Result<(), String> {
    use nexus_core::agent::connect::{connect_unix, RpcMessage, Transport};
    use std::time::Duration;

    let socket = match flag(flags, "--socket") {
        Some(s) => {
            let p = PathBuf::from(s);
            if p.is_relative() {
                return Err(format!("--socket должен быть абсолютным: {}", p.display()));
            }
            p
        }
        None => resolve_socket(flags, &resolve_vault(flags)?)?,
    };
    println!("socket: {}", socket.display());

    // Внятная диагностика ДО connect: нет файла (сервис не запущен) vs не-сокет (мисконфиг).
    use std::os::unix::fs::FileTypeExt;
    match std::fs::symlink_metadata(&socket) {
        Ok(m) if !m.file_type().is_socket() => {
            return Err(format!(
                "путь {} существует, но это НЕ сокет (мисконфиг --socket?)",
                socket.display()
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "сокет {} не найден — сервис не запущен? (`nexus deploy local --apply`)",
                socket.display()
            ))
        }
        _ => {}
    }

    let rt = tokio::runtime::Runtime::new().map_err(|e| format!("tokio: {e}"))?;
    rt.block_on(async {
        let transport = match connect_unix(&socket).await {
            Ok(t) => t,
            Err(e) => {
                return Err(format!(
                "агент НЕдоступен на {} ({e}). Запущен ли сервис? (`nexus deploy local --apply`)",
                socket.display()
            ))
            }
        };
        transport
            .send(RpcMessage::request(
                1,
                "initialize",
                serde_json::json!({ "supportedVersions": ["1.0"] }),
            ))
            .await
            .map_err(|_| "не удалось отправить initialize (сокет закрылся)".to_string())?;
        let resp = tokio::time::timeout(Duration::from_secs(5), transport.recv())
            .await
            .map_err(|_| "таймаут ответа initialize".to_string())?
            .ok_or_else(|| "сокет закрыт без ответа".to_string())?;
        match resp {
            RpcMessage::Response { result: Ok(v), .. } => {
                let ver = v.get("version").and_then(|x| x.as_str()).unwrap_or("?");
                println!("✓ агент ДОСТУПЕН, протокол v{ver}");
                Ok(())
            }
            RpcMessage::Response { result: Err(e), .. } => {
                Err(format!("агент ответил ошибкой: {} ({})", e.message, e.code))
            }
            other => Err(format!("неожиданный ответ: {other:?}")),
        }
    })
}

#[cfg(not(unix))]
fn cmd_status(_flags: &[&str]) -> Result<(), String> {
    Err("status по AF_UNIX доступен только на Unix (на этой ОС коннектор не поддержан)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_parsing() {
        let f = vec!["--vault", "/a", "--apply", "--socket", "/s"];
        assert_eq!(flag(&f, "--vault"), Some("/a"));
        assert_eq!(flag(&f, "--socket"), Some("/s"));
        assert_eq!(flag(&f, "--missing"), None);
        assert!(has_flag(&f, "--apply"));
        assert!(!has_flag(&f, "--nope"));
        // ключ без значения (в конце) → None, не паника.
        assert_eq!(flag(&["--vault"], "--vault"), None);
        // за ключом другой флаг → НЕ значение (`--vault --apply` не делает vault="--apply").
        assert_eq!(flag(&["--vault", "--apply"], "--vault"), None);
    }

    #[test]
    fn socket_default_under_vault_nexus() {
        let s = resolve_socket(&[], Path::new("/home/u/vault")).unwrap();
        assert_eq!(s, PathBuf::from("/home/u/vault/.nexus/agentd.sock"));
        let s2 = resolve_socket(&["--socket", "/tmp/x.sock"], Path::new("/v")).unwrap();
        assert_eq!(s2, PathBuf::from("/tmp/x.sock"));
        // relative --socket отвергается.
        assert!(resolve_socket(&["--socket", "rel.sock"], Path::new("/v")).is_err());
    }

    #[test]
    fn path_chars_rejects_control() {
        assert!(validate_path_chars(Path::new("/ok/path"), "x").is_ok());
        assert!(validate_path_chars(Path::new("/bad\npath"), "x").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_tcc_warns_on_restricted_paths() {
        let mk = |vault: &str| DeployConfig {
            vault: PathBuf::from(vault),
            agentd_bin: PathBuf::from("/usr/local/bin/nexus-agentd"),
            socket: PathBuf::from("/x"),
            log_dir: PathBuf::from("/x"),
        };
        assert!(macos_tcc_warning(&mk("/Users/u/Documents/vault")).is_some());
        assert!(macos_tcc_warning(&mk("/private/tmp/vault")).is_some());
        assert!(macos_tcc_warning(&mk("/Users/u/.nexus/vault")).is_none());
    }
}
