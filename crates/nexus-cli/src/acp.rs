//! `nexus acp` (ACP-2) — ACP **СЕРВЕР** по stdio: тонкая обёртка над ядром
//! [`nexus_core::agent::connect::acp::serve_acp`]. Внешний ACP-клиент (Zed/JetBrains или наш `AcpClient`)
//! спавнит `nexus acp --vault <P>` и драйвит прогон Castor по line-delimited JSON-RPC 2.0 поверх наших
//! stdin/stdout. Инверсия `nexus agent` (тот рисует поток в терминал; этот говорит по протоколу).
//!
//! **SAFE BY DEFAULT (fail-closed):** без `--actuator` — инструментов записи нет (пустой реестр, B7),
//! vault НЕ трогается, гейт не зовётся;
//! автономия `confirm` (каждая правка → `session/request_permission` клиенту; нет явного allow → отказ).
//! `--auto` авто-применяет ЛИШЬ Auto-тир; Confirm-тир (риск) всё равно требует явного разрешения клиента.
//! НЕТ `--yes`/ApproveAll (внешний клиент — единственный аппрувер; авто-одобрение по протоколу побило бы гейт).
//!
//! **stdout — ИСКЛЮЧИТЕЛЬНО канал протокола.** Любой `println!` в stdout испортит JSON-RPC → всё
//! логирование/баннеры идут в stderr (или `tracing`).

use std::path::PathBuf;
use std::sync::Arc;

use nexus_core::actuator::OVERWRITE_THRESHOLD;
use nexus_core::agent::connect::acp::server::StdinStdoutTransport;
use nexus_core::agent::connect::acp::{serve_acp, AcpServerConfig};
use nexus_core::ai::AiConfig;

use crate::agent::build_deps;

/// Подкоманда `nexus acp --vault P [--actuator] [--auto]`. Разбирает флаги, строит рантайм и поднимает
/// ACP-сервер на реальных stdin/stdout. Блокирует до EOF stdin (родитель закрыл пайп).
pub(crate) fn cmd_acp(args: &[&str]) -> Result<(), String> {
    if args.iter().any(|a| matches!(*a, "--help" | "-h")) {
        print_acp_help();
        return Ok(());
    }
    // Неизвестные флаги (кроме --vault <val>) отвергаем — паритет строгости с `nexus agent`.
    if let Some(bad) = unknown_flag(args) {
        return Err(format!(
            "неизвестный флаг {bad} (поддержаны: --vault, --actuator, --auto, --help)"
        ));
    }
    let actuator = crate::has_flag(args, "--actuator");
    let auto = crate::has_flag(args, "--auto");
    let vault = crate::resolve_vault(args)?;

    // --auto без --actuator — no-op (предлагать нечего): честно предупреждаем (зеркало `nexus agent`).
    if !actuator && auto {
        eprintln!("nexus acp: --auto без --actuator ничего не меняет (актуатор выключен)");
    }

    let rt = tokio::runtime::Runtime::new().map_err(|e| format!("tokio: {e}"))?;
    rt.block_on(run_acp(vault, actuator, auto))
}

/// Известные булевы флаги (без значения).
const BOOL_FLAGS: &[&str] = &["--actuator", "--auto", "--help", "-h"];

/// Находит первый НЕизвестный `--флаг` (пропуская `--vault <val>` и булевы). `None` — всё валидно.
fn unknown_flag<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--vault" => i += 2, // флаг + значение
            t if BOOL_FLAGS.contains(&t) => i += 1,
            t if t.starts_with('-') => return Some(t),
            _ => i += 1, // позиционный (ACP не принимает задачу из argv) — игнор, не ошибка
        }
    }
    None
}

/// Поднимает ACP-сервер: shared `build_deps` → `AcpServerConfig` → `serve_acp` на реальных stdin/stdout.
async fn run_acp(root: PathBuf, actuator: bool, auto: bool) -> Result<(), String> {
    let deps = build_deps(root).await?;
    let autonomy = if auto { "auto" } else { "confirm" };

    // Баннер — в STDERR (stdout — провод протокола).
    eprintln!(
        "nexus acp (ACP-server) · vault={} · model={} · actuator={} · autonomy={autonomy} · \
         permissions=fail-closed\nговорю JSON-RPC 2.0 по stdin/stdout; жду ACP-клиента (EOF stdin → выход).",
        deps.canon_root.display(),
        deps.model,
        if actuator { "ON" } else { "OFF (stub)" },
    );

    let cfg = AcpServerConfig {
        provider: deps.provider,
        writer: deps.db.writer().clone(),
        reader: deps.db.reader().clone(),
        canon_root: deps.canon_root,
        actuator_enabled: actuator,
        autonomy: autonomy.to_string(),
        overwrite_threshold: OVERWRITE_THRESHOLD,
        blast_cap: AiConfig::DEFAULT_BLAST_RADIUS_CAP,
        context_window: deps.context_window,
        model: deps.model,
    };
    serve_acp(Arc::new(StdinStdoutTransport::new()), Arc::new(cfg)).await;
    Ok(())
}

fn print_acp_help() {
    eprintln!(
        "nexus acp — ACP-СЕРВЕР по stdio (внешний ACP-клиент драйвит прогон Castor)\n\n\
         ИСПОЛЬЗОВАНИЕ:\n  \
         nexus acp --vault PATH [--actuator] [--auto]\n\n\
         Клиент (Zed/JetBrains или наш AcpClient) спавнит эту команду и говорит line-delimited\n  \
         JSON-RPC 2.0 (ACP v1) по её stdin/stdout: initialize → session/new → session/prompt → …\n\n\
         ФЛАГИ:\n  \
         --vault PATH   корень vault (по умолчанию — текущий каталог; client cwd ИГНОРИРУЕТСЯ)\n  \
         --actuator     включить живые правки vault через гейт; без него правок нет (vault не трогается)\n  \
         --auto         автономия `auto`: Auto-тир применяется сам; Confirm-тир всё равно спрашивает клиента\n  \
         -h, --help     эта справка\n\n\
         БЕЗОПАСНОСТЬ (по умолчанию): актуатор ВЫКЛ, автономия confirm, permission fail-closed —\n  \
         каждая правка идёт клиенту как session/request_permission; нет явного allow → отказ.\n  \
         НЕТ --yes/auto-approve по протоколу. Нужен .nexus/local.json с ai.chat (url/model).\n  \
         ВСЁ логирование — в stderr (stdout — канал протокола)."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_flag_detected() {
        assert_eq!(unknown_flag(&["--bogus"]), Some("--bogus"));
        assert_eq!(
            unknown_flag(&["--vault", "/v", "--actuator", "--auto"]),
            None
        );
        // --vault значение НЕ трактуется как флаг.
        assert_eq!(unknown_flag(&["--vault", "--actuator"]), None);
        assert_eq!(unknown_flag(&[]), None);
    }

    /// Дефолтная поза: без --actuator → actuator OFF; без --auto → autonomy "confirm".
    #[test]
    fn default_posture_safe() {
        let args: &[&str] = &["--vault", "/v"];
        assert!(
            !crate::has_flag(args, "--actuator"),
            "actuator OFF по умолчанию"
        );
        assert!(!crate::has_flag(args, "--auto"), "auto OFF по умолчанию");
        let autonomy = if crate::has_flag(args, "--auto") {
            "auto"
        } else {
            "confirm"
        };
        assert_eq!(autonomy, "confirm", "autonomy=confirm по умолчанию");
    }

    #[test]
    fn flags_parsed() {
        let args: &[&str] = &["--vault", "/v", "--actuator", "--auto"];
        assert!(crate::has_flag(args, "--actuator"));
        assert!(crate::has_flag(args, "--auto"));
        let autonomy = if crate::has_flag(args, "--auto") {
            "auto"
        } else {
            "confirm"
        };
        assert_eq!(autonomy, "auto");
    }
}
