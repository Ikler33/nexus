//! `nexus` — CLI деплоя/управления агент-сервисом `nexus-agentd` (PROD-v1, item 4).
//!
//! Команды:
//! - `nexus deploy local [--vault P] [--socket P] [--agentd P] [--apply]` — bootstrap `.nexus` + рендер
//!   сервис-юнита (launchd/systemd --user), который запускает `nexus-agentd <vault>` с
//!   `NEXUS_AGENTD_CONNECT_SOCKET`. **Safe default — печать ПЛАНА**; реальная установка только под `--apply`.
//! - `nexus deploy remote --host user@host --binary P [...] [--apply]` — деплой agentd на удалённый
//!   Linux-хост (systemd --user) через `ssh`/`scp`: `scp` бинаря → юнит → `systemctl --user enable --now`.
//!   Целевой хост — риг с локальным LLM (192.168.0.31). **Safe default — печать ПЛАНА**; ssh/scp под `--apply`.
//! - `nexus deploy docker --vault P [--image N] [--name N] [--user uid:gid] [--build [--context P]]
//!   [--apply] [--force]` — запуск agentd в Docker-контейнере (образ — `Dockerfile` в корне репо):
//!   vault-том + AF_UNIX-сокет на нём. **Safe default — печать ПЛАНА**; `docker build`/`docker run` под
//!   `--apply` (на macOS `--apply` заблокирован без `--force` — virtiofs не пробрасывает сокет).
//!   `undeploy docker` — stop+rm.
//! - `nexus status [--socket P] [--vault P]` — проба коннектора: подключиться к AF_UNIX-сокету и сделать
//!   `initialize` → доступность + версия протокола.
//! - `nexus undeploy [--apply]` — остановить + удалить локальный сервис-юнит (план / `--apply`).
//! - `nexus undeploy remote --host user@host [--remote-home P] [--apply]` — снять удалённый systemd
//!   --user сервис (disable+rm юнита; бинарь/vault не трогает). `undeploy docker` — stop+rm контейнера.
//!
//! Минимум зависимостей (без clap — ручной разбор, как у `nexus-agentd`); сетевого egress нет (только
//! локальный AF_UNIX для `status`).

mod acp;
mod agent;
mod service;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use service::{detect_kind, DeployConfig, DockerConfig, RemoteConfig, RemotePlan, RemoteStep};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().map(String::as_str).collect();
    match rest.as_slice() {
        [] | ["help"] | ["--help"] | ["-h"] => {
            print_help();
            ExitCode::SUCCESS
        }
        ["deploy", "local", flags @ ..] => run(cmd_deploy_local(flags)),
        ["deploy", "remote", flags @ ..] => run(cmd_deploy_remote(flags)),
        ["deploy", "docker", flags @ ..] => run(cmd_deploy_docker(flags)),
        ["agent", rest @ ..] => run(agent::cmd_agent(rest)),
        ["acp", rest @ ..] => run(acp::cmd_acp(rest)),
        ["status", flags @ ..] => run(cmd_status(flags)),
        // `undeploy docker`/`undeploy remote` ДО общего `undeploy` (иначе под-команда утечёт во флаги
        // launchd/systemd-выгрузки).
        ["undeploy", "docker", flags @ ..] => run(cmd_undeploy_docker(flags)),
        ["undeploy", "remote", flags @ ..] => run(cmd_undeploy_remote(flags)),
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
         agent [--vault P] \"<задача>\"\n      \
         Запустить агента в терминале (one-shot, без записи в vault). `nexus agent --help` — детали.\n  \
         acp --vault P [--actuator] [--auto]\n      \
         ACP-сервер по stdio (внешний ACP-клиент драйвит Castor). Safe by default: actuator OFF,\n      \
         автономия confirm, permission fail-closed. `nexus acp --help` — детали.\n  \
         deploy local [--vault P] [--socket P] [--agentd P] [--apply]\n      \
         Развернуть agentd локальным сервисом (launchd/systemd). Без --apply — печать плана.\n  \
         deploy remote --host user@host --binary P [--remote-vault P] [--remote-socket P]\n               \
         [--remote-home P] [--apply]\n      \
         Развернуть agentd на удалённом Linux-хосте (systemd --user) через ssh/scp. Без --apply — план.\n  \
         deploy docker --vault P [--image N] [--name N] [--user uid:gid] [--build [--context P]]\n               \
         [--apply] [--force]\n      \
         Запустить agentd в Docker-контейнере (vault-том + AF_UNIX-сокет). Без --apply — план.\n  \
         status [--socket P] [--vault P]\n      Проверить доступность агента (initialize по AF_UNIX).\n  \
         undeploy [--apply]            Остановить и удалить локальный сервис.\n  \
         undeploy remote --host user@host [--remote-home P] [--apply]\n      \
         Снять удалённый systemd --user сервис (disable+rm юнита; бинарь/vault не трогает).\n  \
         undeploy docker [--name N] [--apply]   Остановить и удалить контейнер.\n\n\
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

// ── deploy remote (DEPLOY-2) ────────────────────────────────────────────────────────────────────

/// Символы, недопустимые в УДАЛЁННОМ пути/хосте — они встраиваются в shell-команду `ssh <target> <cmd>`
/// БЕЗ экранирования, поэтому любой shell-метасимвол = инъекция. Удалённые пути на риге чистые
/// (`/home/artan/...`), так что строгий allowlist не мешает легитимным кейсам.
const SHELL_META: &[char] = &[
    '\'', '"', '\\', '$', '`', ';', '|', '&', '<', '>', '(', ')', '{', '}', '[', ']', '*', '?',
    '~', '#', '!', '\n', '\r', '\0', '\t', ' ',
];

/// Валидирует «чистый» удалённый АБСОЛЮТНЫЙ путь, безопасный для встраивания в ssh/scp-команду без
/// shell-экранирования: непустой, абсолютный (`/`), без shell-метасимволов и управляющих символов.
fn validate_remote_path(s: &str, what: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err(format!("{what} пуст"));
    }
    if !s.starts_with('/') {
        return Err(format!(
            "{what} должен быть абсолютным (начинаться с /): {s}"
        ));
    }
    if let Some(c) = s.chars().find(|c| SHELL_META.contains(c) || c.is_control()) {
        return Err(format!(
            "{what} содержит недопустимый для ssh-команды символ {c:?}: {s}"
        ));
    }
    Ok(())
}

/// Валидирует имя удалённого пользователя (ssh + `loginctl enable-linger <user>`): непустое, только
/// `[A-Za-z0-9._-]`, не начинается с `-` (иначе ssh примет за флаг).
fn validate_remote_user(u: &str) -> Result<(), String> {
    if u.is_empty() || u.starts_with('-') {
        return Err(format!("недопустимое имя пользователя: {u:?}"));
    }
    if !u
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!("недопустимое имя пользователя: {u:?}"));
    }
    Ok(())
}

/// Валидирует удалённый хост (IPv4/DNS) строгим ALLOWLIST `[A-Za-z0-9.-]`, не начинается с `-`.
/// Allowlist (а не blocklist) закрывает тихие мис-таргеты: `@` (мис-парс `user@host`), `:` (порт без
/// `-P` молча игнорируется), `%`/`,`/`=` (ssh-токены/опции). IPv6-литералы (`:`/`[]`) не поддержаны —
/// используйте hostname или ssh-config-алиас.
fn validate_remote_host(h: &str) -> Result<(), String> {
    if h.is_empty() || h.starts_with('-') {
        return Err(format!("недопустимый хост: {h:?}"));
    }
    if let Some(c) = h
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
    {
        return Err(format!(
            "хост содержит недопустимый символ {c:?} (разрешены буквы/цифры/./-, IPv6-литералы — \
             через hostname/ssh-config): {h}"
        ));
    }
    Ok(())
}

fn cmd_deploy_remote(flags: &[&str]) -> Result<(), String> {
    let host_spec = flag(flags, "--host").ok_or("укажите --host user@host")?;
    let (user, host) = host_spec
        .split_once('@')
        .ok_or_else(|| format!("--host должен быть в форме user@host: {host_spec}"))?;
    validate_remote_user(user)?;
    validate_remote_host(host)?;

    let local_binary = PathBuf::from(
        flag(flags, "--binary")
            .ok_or("укажите --binary <путь к Linux-бинарю nexus-agentd для удалённого хоста>")?,
    );
    validate_path_chars(&local_binary, "binary")?;
    if !local_binary.is_file() {
        return Err(format!(
            "локальный бинарь не найден: {} (соберите под Linux: \
             cargo build --release -p nexus-agentd --target x86_64-unknown-linux-gnu)",
            local_binary.display()
        ));
    }

    // Удалённые пути — ВСЕГДА POSIX (`posix_join`), даже если CLI запущен на Windows-хосте.
    let remote_home = flag(flags, "--remote-home")
        .map(PathBuf::from)
        .unwrap_or_else(|| service::default_remote_home(user));
    let remote_vault = flag(flags, "--remote-vault")
        .map(PathBuf::from)
        .unwrap_or_else(|| service::posix_join(&remote_home, ".nexus/vault"));
    let remote_socket = flag(flags, "--remote-socket")
        .map(PathBuf::from)
        .unwrap_or_else(|| service::posix_join(&remote_vault, ".nexus/agentd.sock"));

    // Удалённые пути встраиваются в ssh-команды без экранирования → строгая валидация.
    for (p, what) in [
        (&remote_home, "--remote-home"),
        (&remote_vault, "--remote-vault"),
        (&remote_socket, "--remote-socket"),
    ] {
        validate_remote_path(&p.to_string_lossy(), what)?;
    }

    let cfg = RemoteConfig {
        user: user.to_string(),
        host: host.to_string(),
        remote_home,
        local_binary,
        remote_vault,
        remote_socket,
    };
    let plan = service::remote_plan(&cfg);
    let apply = has_flag(flags, "--apply");

    println!(
        "=== nexus deploy remote (systemd --user @ {}) ===",
        plan.target
    );
    println!("binary(local):  {}", cfg.local_binary.display());
    println!("binary(remote): {}", plan.remote_bin.display());
    println!("vault(remote):  {}", cfg.remote_vault.display());
    println!("socket(remote): {}", cfg.remote_socket.display());
    println!("unit(remote):   {}", plan.remote_unit_path.display());
    println!("\n--- содержимое юнита ---\n{}", plan.unit_content);
    println!("--- шаги ---");
    for s in &plan.steps {
        println!("  {}", describe_remote_step(s, &cfg, &plan));
    }

    if !apply {
        println!("\n(dry-run — план НЕ применён; повторите с --apply для удалённой установки)");
        println!(
            "ПРЕДУСЛОВИЯ: ssh-доступ к {} (ключ/ssh-agent), на хосте — systemd --user.",
            plan.target
        );
        return Ok(());
    }
    apply_remote_plan(&cfg, &plan)
}

/// Человекочитаемое описание шага для печати плана.
fn describe_remote_step(step: &RemoteStep, cfg: &RemoteConfig, plan: &RemotePlan) -> String {
    match step {
        RemoteStep::Run { cmd, best_effort } => {
            let tail = if *best_effort { "  (best-effort)" } else { "" };
            format!("ssh {} {cmd:?}{tail}", plan.target)
        }
        RemoteStep::PutBinary => format!(
            "scp {} {}:{}",
            cfg.local_binary.display(),
            plan.target,
            plan.remote_bin.display()
        ),
        RemoteStep::PutUnit => format!(
            "scp <временный-юнит> {}:{}",
            plan.target,
            plan.remote_unit_path.display()
        ),
    }
}

/// Исполняет [`RemotePlan`] через `ssh`/`scp` (наследует stdio — интерактивный ввод пароля/passphrase
/// работает). Не-`best_effort` сбой прерывает деплой. Аутентификация — на стороне ssh (ключ/agent/конфиг).
fn apply_remote_plan(cfg: &RemoteConfig, plan: &RemotePlan) -> Result<(), String> {
    use std::io::Write;
    use std::process::Command;
    // Временный юнит — детерминированное имя по PID (без Date/random); чистим в конце независимо от исхода.
    let tmp_unit =
        std::env::temp_dir().join(format!("nexus-agentd-{}.service", std::process::id()));
    let cleanup = |t: &Path| {
        let _ = std::fs::remove_file(t);
    };

    for step in &plan.steps {
        let result: Result<(), String> = match step {
            RemoteStep::Run { cmd, best_effort } => {
                let st = Command::new("ssh")
                    .arg(&plan.target)
                    .arg(cmd)
                    .status()
                    .map_err(|e| format!("запуск ssh не удался: {e}"))?;
                if st.success() {
                    println!("  ✓ ssh {}: {cmd}", plan.target);
                    Ok(())
                } else if *best_effort {
                    println!(
                        "  ⚠ (best-effort) ssh {}: {cmd} → код {}",
                        plan.target,
                        st.code().unwrap_or(-1)
                    );
                    Ok(())
                } else {
                    // Подсказка для самого частого фейла «свежего хоста»: `systemctl --user` падает
                    // с «Failed to connect to bus», если у пользователя нет активной сессии и линджер
                    // не включился (best-effort `loginctl enable-linger` мог не пройти без root/polkit).
                    let hint = if cmd.contains("systemctl --user") {
                        format!(
                            "\n  ⓘ вероятная причина: у пользователя {} нет user-bus — включите линджер \
                             с правами: `ssh {} 'sudo loginctl enable-linger {}'` и повторите --apply \
                             (бинарь и юнит уже доставлены).",
                            cfg.user, plan.target, cfg.user
                        )
                    } else {
                        String::new()
                    };
                    Err(format!(
                        "шаг провалился (код {}): ssh {} {cmd}{hint}",
                        st.code().unwrap_or(-1),
                        plan.target
                    ))
                }
            }
            RemoteStep::PutBinary => {
                let remote = format!("{}:{}", plan.target, plan.remote_bin.display());
                let st = Command::new("scp")
                    .arg(&cfg.local_binary)
                    .arg(&remote)
                    .status()
                    .map_err(|e| format!("запуск scp не удался: {e}"))?;
                if st.success() {
                    println!("  ✓ scp бинаря → {remote}");
                    Ok(())
                } else {
                    Err(format!(
                        "scp бинаря провалился (код {})",
                        st.code().unwrap_or(-1)
                    ))
                }
            }
            RemoteStep::PutUnit => {
                // Symlink-safe запись в общий /tmp: снимаем возможный устаревший симлинк (unlink НЕ идёт
                // по ссылке), затем создаём с O_EXCL (`create_new` — не следует по предсуществующей ссылке).
                let _ = std::fs::remove_file(&tmp_unit);
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp_unit)
                    .map_err(|e| format!("создание temp-юнита {}: {e}", tmp_unit.display()))?;
                f.write_all(plan.unit_content.as_bytes())
                    .map_err(|e| format!("запись temp-юнита {}: {e}", tmp_unit.display()))?;
                drop(f);
                let remote = format!("{}:{}", plan.target, plan.remote_unit_path.display());
                let st = Command::new("scp")
                    .arg(&tmp_unit)
                    .arg(&remote)
                    .status()
                    .map_err(|e| format!("запуск scp не удался: {e}"))?;
                if st.success() {
                    println!("  ✓ scp юнита → {remote}");
                    Ok(())
                } else {
                    Err(format!(
                        "scp юнита провалился (код {})",
                        st.code().unwrap_or(-1)
                    ))
                }
            }
        };
        if let Err(e) = result {
            cleanup(&tmp_unit);
            return Err(e);
        }
    }
    cleanup(&tmp_unit);
    println!(
        "\n✓ удалённый деплой применён. Проверка:\n  \
         ssh {} 'export XDG_RUNTIME_DIR=/run/user/$(id -u); systemctl --user status {}'",
        plan.target,
        service::SYSTEMD_UNIT
    );
    Ok(())
}

/// `undeploy remote` — симметрия `deploy remote`: снимает удалённый systemd --user сервис (disable+rm
/// юнита, daemon-reload). Бинарь/vault НЕ трогает (паритет с локальным `undeploy`).
fn cmd_undeploy_remote(flags: &[&str]) -> Result<(), String> {
    let host_spec = flag(flags, "--host").ok_or("укажите --host user@host")?;
    let (user, host) = host_spec
        .split_once('@')
        .ok_or_else(|| format!("--host должен быть в форме user@host: {host_spec}"))?;
    validate_remote_user(user)?;
    validate_remote_host(host)?;

    let remote_home = flag(flags, "--remote-home")
        .map(PathBuf::from)
        .unwrap_or_else(|| service::default_remote_home(user));
    validate_remote_path(&remote_home.to_string_lossy(), "--remote-home")?;

    let (target, unit_path, steps) = service::remote_undeploy_plan(user, host, &remote_home);
    let apply = has_flag(flags, "--apply");

    println!("=== nexus undeploy remote ({target}) ===");
    println!("unit(remote): {}", unit_path.display());
    println!("--- шаги (все best-effort) ---");
    for s in &steps {
        if let RemoteStep::Run { cmd, .. } = s {
            println!("  ssh {target} {cmd:?}");
        }
    }
    if !apply {
        println!("\n(dry-run — план НЕ применён; повторите с --apply)");
        return Ok(());
    }
    // Все шаги best-effort: снятие отсутствующего сервиса не должно валить undeploy.
    for s in &steps {
        let RemoteStep::Run { cmd, .. } = s else {
            continue;
        };
        match std::process::Command::new("ssh")
            .arg(&target)
            .arg(cmd)
            .status()
        {
            Ok(st) if st.success() => println!("  ✓ ssh {target}: {cmd}"),
            Ok(st) => println!("  ⚠ ssh {target}: {cmd} → код {}", st.code().unwrap_or(-1)),
            Err(e) => println!("  ⚠ ssh {target}: {cmd} → {e}"),
        }
    }
    println!("\n✓ undeploy remote применён (бинарь/vault не тронуты)");
    Ok(())
}

// ── deploy docker (DEPLOY-3) ────────────────────────────────────────────────────────────────────

/// Имя Docker-образа: `[A-Za-z0-9._:/-]`, непустое, не с `-` (иначе docker примет за флаг). Покрывает
/// `repo/name:tag` и `registry.host/name:tag`.
fn validate_image_name(s: &str) -> Result<(), String> {
    if s.is_empty() || s.starts_with('-') {
        return Err(format!("недопустимое имя образа: {s:?}"));
    }
    if let Some(c) = s
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-')))
    {
        return Err(format!(
            "имя образа содержит недопустимый символ {c:?}: {s}"
        ));
    }
    Ok(())
}

/// Имя контейнера: docker допускает `[a-zA-Z0-9][a-zA-Z0-9_.-]*`; требуем то же (и не с `-`).
fn validate_container_name(s: &str) -> Result<(), String> {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => {
            return Err(format!(
                "имя контейнера должно начинаться с буквы/цифры: {s:?}"
            ))
        }
    }
    if let Some(c) = chars.find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))) {
        return Err(format!(
            "имя контейнера содержит недопустимый символ {c:?}: {s}"
        ));
    }
    Ok(())
}

/// Значение `docker run --user`: `uid`, `uid:gid`, `name` или `name:group`. Allowlist `[A-Za-z0-9_:.-]`,
/// не пустое, не с `-` (иначе docker примет за флаг).
fn validate_docker_user(s: &str) -> Result<(), String> {
    if s.is_empty() || s.starts_with('-') {
        return Err(format!("недопустимое значение --user: {s:?}"));
    }
    if let Some(c) = s
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-')))
    {
        return Err(format!("--user содержит недопустимый символ {c:?}: {s}"));
    }
    Ok(())
}

fn cmd_deploy_docker(flags: &[&str]) -> Result<(), String> {
    let image = flag(flags, "--image").unwrap_or("nexus-agentd:local");
    validate_image_name(image)?;
    let container_name = flag(flags, "--name").unwrap_or("nexus-agentd");
    validate_container_name(container_name)?;

    // --vault ОБЯЗАТЕЛЕН (его bind-mount'им в /vault): без него resolve_vault взял бы cwd → случайный
    // каталог смонтировался бы как vault (footgun). Требуем явно, как `deploy remote` требует --host.
    if flag(flags, "--vault").is_none() {
        return Err("укажите --vault <путь к vault> (его монтируем в /vault контейнера)".into());
    }
    let host_vault = resolve_vault(flags)?;
    validate_path_chars(&host_vault, "vault")?;

    // --user uid[:gid] → docker run --user (контейнер пишет bind-mount vault + сокет, доступный хосту).
    let run_user = match flag(flags, "--user") {
        Some(u) => {
            validate_docker_user(u)?;
            Some(u.to_string())
        }
        None => None,
    };

    // --build → включить шаг docker build; контекст = --context | cwd (репо с Dockerfile).
    let build_context = if has_flag(flags, "--build") {
        let ctx = flag(flags, "--context")
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir().map_err(|e| format!("cwd: {e}"))?);
        let ctx = ctx
            .canonicalize()
            .map_err(|e| format!("--context {}: {e}", ctx.display()))?;
        if !ctx.join("Dockerfile").is_file() {
            return Err(format!(
                "в контексте сборки нет Dockerfile: {} (укажите --context <корень репо>)",
                ctx.display()
            ));
        }
        Some(ctx)
    } else {
        None
    };

    let cfg = DockerConfig {
        image: image.to_string(),
        container_name: container_name.to_string(),
        host_vault: host_vault.clone(),
        build_context,
        run_user,
    };
    let plan = service::docker_plan(&cfg);
    let apply = has_flag(flags, "--apply");

    println!("=== nexus deploy docker ===");
    println!("image:      {}", cfg.image);
    println!("container:  {}", cfg.container_name);
    println!("vault(host): {}", cfg.host_vault.display());
    println!(
        "build:      {}",
        cfg.build_context
            .as_ref()
            .map(|c| c.display().to_string())
            .unwrap_or_else(|| "(нет — используется существующий образ)".into())
    );
    println!("socket(host): {}", plan.host_socket.display());
    println!("--- команды ---");
    for c in &plan.steps {
        println!("  {}", c.join(" "));
    }
    if cfg.build_context.is_none() {
        eprintln!(
            "\nⓘ образ {} должен существовать (нет --build). Соберите: `nexus deploy docker --vault {} --build`",
            cfg.image,
            cfg.host_vault.display()
        );
    }
    if cfg.run_user.is_none() {
        eprintln!(
            "\nⓘ контейнер бежит под uid 10001 (образ): vault {} должен быть доступен ему на запись, \
             иначе agentd не откроет БД/сокет. Приравняйте к владельцу vault: `--user $(id -u):$(id -g)`.",
            cfg.host_vault.display()
        );
    }
    let on_macos = cfg!(target_os = "macos");
    if on_macos {
        eprintln!(
            "\n⚠ macOS Docker Desktop: AF_UNIX-сокет на bind-mount НЕ пробрасывается через virtiofs — \
             коннектор не подключится. Контейнер-деплой рассчитан на Linux-хост (риг/VPS)."
        );
    }
    if !apply {
        println!("\n(dry-run — план НЕ применён; повторите с --apply)");
        return Ok(());
    }
    // На macOS контейнер запустится, но коннектор недостижим → не ставим заведомо нерабочую конфигурацию
    // (как `deploy local` отказывается ставить нерабочий сервис). Override — `--force` (напр. контейнер
    // только под scheduler без коннектора).
    if on_macos && !has_flag(flags, "--force") {
        return Err(
            "--apply на macOS заблокирован: коннектор по AF_UNIX недостижим (см. предупреждение выше). \
             Деплойте на Linux-хост или повторите с --force, если коннектор не нужен."
                .into(),
        );
    }
    run_cmds_strict(&plan.steps).map_err(|e| {
        format!(
            "{e}\n  ⓘ если контейнер «{}» уже существует — сначала `nexus undeploy docker --name {}`",
            cfg.container_name, cfg.container_name
        )
    })?;
    println!(
        "\n✓ контейнер запущен. Проверка: `nexus status --socket {}`",
        plan.host_socket.display()
    );
    Ok(())
}

fn cmd_undeploy_docker(flags: &[&str]) -> Result<(), String> {
    let container_name = flag(flags, "--name").unwrap_or("nexus-agentd");
    validate_container_name(container_name)?;
    let cmds = service::docker_undeploy_plan(container_name);
    let apply = has_flag(flags, "--apply");

    println!("=== nexus undeploy docker ===");
    println!("container: {container_name}");
    println!("--- команды ---");
    for c in &cmds {
        println!("  {}", c.join(" "));
    }
    if !apply {
        println!("\n(dry-run — повторите с --apply)");
        return Ok(());
    }
    // best-effort: `stop`/`rm` уже-отсутствующего контейнера не должны валить undeploy.
    run_cmds(&cmds);
    println!("✓ undeploy docker применён");
    Ok(())
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

/// Выполняет argv-команды СТРОГО: первый сбой прерывает (Err) — напр. провал `docker build` не должен
/// вести к `docker run` устаревшего/несуществующего образа.
fn run_cmds_strict(cmds: &[Vec<String>]) -> Result<(), String> {
    for c in cmds {
        let Some((prog, rest)) = c.split_first() else {
            continue;
        };
        match std::process::Command::new(prog).args(rest).status() {
            Ok(st) if st.success() => println!("  ✓ {}", c.join(" ")),
            Ok(st) => {
                return Err(format!(
                    "команда провалилась (код {}): {}",
                    st.code().unwrap_or(-1),
                    c.join(" ")
                ))
            }
            Err(e) => return Err(format!("не удалось запустить `{}`: {e}", c.join(" "))),
        }
    }
    Ok(())
}

// ── status ────────────────────────────────────────────────────────────────────────────────────────

/// CONN-4: байт-прежнее сообщение диагностики сокета для `nexus status` по вердикту канона
/// [`classify_socket`]. `None` — путь пригоден (проба продолжается). Тексты специфичны для CLI
/// (упоминают флаг `--socket` и `nexus deploy local --apply`) — потому маппинг ЗДЕСЬ, не в ядре.
#[cfg(unix)]
fn status_socket_diag_err(
    diag: nexus_core::agent::connect::SocketDiag,
    socket: &Path,
) -> Option<String> {
    use nexus_core::agent::connect::SocketDiag;
    match diag {
        SocketDiag::NotSocket => Some(format!(
            "путь {} существует, но это НЕ сокет (мисконфиг --socket?)",
            socket.display()
        )),
        SocketDiag::Missing => Some(format!(
            "сокет {} не найден — сервис не запущен? (`nexus deploy local --apply`)",
            socket.display()
        )),
        SocketDiag::Usable => None,
    }
}

/// CONN-4: байт-прежнее сообщение ошибки пробы `initialize` для `nexus status` (см.
/// [`nexus_core::agent::connect::probe_initialize`]). Ok-ветка (println версии) остаётся на call-site.
#[cfg(unix)]
fn status_probe_err(err: nexus_core::agent::connect::ProbeError) -> String {
    use nexus_core::agent::connect::ProbeError;
    match err {
        ProbeError::Message(m) => m,
        ProbeError::Rpc(e) => format!("агент ответил ошибкой: {} ({})", e.message, e.code),
        ProbeError::Unexpected(other) => format!("неожиданный ответ: {other:?}"),
    }
}

#[cfg(unix)]
fn cmd_status(flags: &[&str]) -> Result<(), String> {
    use nexus_core::agent::connect::{classify_socket, connect_unix, probe_initialize};
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

    // Внятная диагностика ДО connect: нет файла (сервис не запущен) vs не-сокет (мисконфиг) — ЕДИНАЯ
    // классификация в ядре (`classify_socket`), байт-прежний текст маппится тут (`status_socket_diag_err`).
    if let Some(e) = status_socket_diag_err(classify_socket(&socket), &socket) {
        return Err(e);
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
        match probe_initialize(&transport, Duration::from_secs(5)).await {
            Ok(ver) => {
                println!("✓ агент ДОСТУПЕН, протокол v{ver}");
                Ok(())
            }
            Err(e) => Err(status_probe_err(e)),
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

    // Unix-семантика путей: `/home/...` АБСОЛЮТНО только на Unix (на Windows нет буквы диска → relative,
    // и resolve_socket его отвергает). Деплой-CLI всё равно Unix-only (launchd/systemd/AF_UNIX).
    #[cfg(unix)]
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

    // CONN-4/R-12b: характеризация БАЙТ-ПРЕЖНИХ текстов `nexus status` после дедупа socket-диагностики
    // (канон `classify_socket`/`probe_initialize` в ядре; тексты — тут). Пинят точные строки.
    #[cfg(unix)]
    #[test]
    fn status_socket_diag_messages_byte_exact() {
        use nexus_core::agent::connect::SocketDiag;
        let p = Path::new("/v/.nexus/agentd.sock");
        assert_eq!(
            status_socket_diag_err(SocketDiag::NotSocket, p).unwrap(),
            "путь /v/.nexus/agentd.sock существует, но это НЕ сокет (мисконфиг --socket?)"
        );
        assert_eq!(
            status_socket_diag_err(SocketDiag::Missing, p).unwrap(),
            "сокет /v/.nexus/agentd.sock не найден — сервис не запущен? (`nexus deploy local --apply`)"
        );
        assert!(status_socket_diag_err(SocketDiag::Usable, p).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn status_probe_err_messages_byte_exact() {
        use nexus_core::agent::connect::{ProbeError, RpcError, RpcMessage};
        assert_eq!(
            status_probe_err(ProbeError::Message("таймаут ответа initialize".to_string())),
            "таймаут ответа initialize"
        );
        assert_eq!(
            status_probe_err(ProbeError::Rpc(RpcError::version_incompatible())),
            "агент ответил ошибкой: protocol version incompatible (-32001)"
        );
        let other = RpcMessage::notification("agent/event", serde_json::json!({"type": "final"}));
        assert_eq!(
            status_probe_err(ProbeError::Unexpected(other.clone())),
            format!("неожиданный ответ: {other:?}")
        );
    }

    #[test]
    fn remote_path_validation() {
        assert!(validate_remote_path("/home/artan/.nexus/bin", "x").is_ok());
        assert!(validate_remote_path("relative/path", "x").is_err());
        assert!(validate_remote_path("/home/with space", "x").is_err());
        assert!(validate_remote_path("/home/$(whoami)", "x").is_err());
        assert!(validate_remote_path("/home/a;rm -rf /", "x").is_err());
        assert!(validate_remote_path("/home/a`x`", "x").is_err());
        assert!(validate_remote_path("/home/a|b", "x").is_err());
        assert!(validate_remote_path("", "x").is_err());
    }

    #[test]
    fn remote_user_validation() {
        assert!(validate_remote_user("artan").is_ok());
        assert!(validate_remote_user("root").is_ok());
        assert!(validate_remote_user("user.name-1_2").is_ok());
        assert!(validate_remote_user("").is_err());
        assert!(validate_remote_user("-flag").is_err());
        assert!(validate_remote_user("a b").is_err());
        assert!(validate_remote_user("user;rm").is_err());
    }

    #[test]
    fn remote_host_validation() {
        assert!(validate_remote_host("192.168.0.31").is_ok());
        assert!(validate_remote_host("rig.local").is_ok());
        assert!(validate_remote_host("").is_err());
        assert!(validate_remote_host("-x").is_err());
        assert!(validate_remote_host("a b").is_err());
        assert!(validate_remote_host("h$(x)").is_err());
        assert!(validate_remote_host("h;rm").is_err());
        // allowlist закрывает тихие мис-таргеты: @ (мис-парс user@host), : (порт), , (host-list).
        assert!(validate_remote_host("a@b").is_err());
        assert!(validate_remote_host("h:22").is_err());
        assert!(validate_remote_host("h,x").is_err());
    }

    #[test]
    fn image_name_validation() {
        assert!(validate_image_name("nexus-agentd:local").is_ok());
        assert!(validate_image_name("ghcr.io/ikler33/nexus-agentd:1.0").is_ok());
        assert!(validate_image_name("").is_err());
        assert!(validate_image_name("-x").is_err());
        assert!(validate_image_name("img;rm").is_err());
        assert!(validate_image_name("a b").is_err());
    }

    #[test]
    fn container_name_validation() {
        assert!(validate_container_name("nexus-agentd").is_ok());
        assert!(validate_container_name("agent_1.test").is_ok());
        assert!(validate_container_name("").is_err());
        assert!(validate_container_name("-x").is_err());
        assert!(validate_container_name(".x").is_err());
        assert!(validate_container_name("a/b").is_err());
        assert!(validate_container_name("a b").is_err());
    }

    #[test]
    fn docker_user_validation() {
        assert!(validate_docker_user("1000").is_ok());
        assert!(validate_docker_user("1000:1000").is_ok());
        assert!(validate_docker_user("nexus:nexus").is_ok());
        assert!(validate_docker_user("").is_err());
        assert!(validate_docker_user("-1").is_err());
        assert!(validate_docker_user("1000 1000").is_err());
        assert!(validate_docker_user("u;rm").is_err());
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
