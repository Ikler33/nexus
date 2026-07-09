//! exec_child — ЕДИНСТВЕННОЕ место реального исполнения exec-команды агента (SANDBOX-6c-2, §5.2).
//!
//! **КЛЮЧЕВАЯ ИНВЕРСИЯ §5.2:** host РЕШАЕТ (classify→approval→ledger, `exec_host`), КОНТЕЙНЕР ИСПОЛНЯЕТ.
//! Этот модуль работает ВНУТРИ `--network=none` песочницы (vault `:ro`, cap-drop, no-new-privileges, no NIC,
//! пустое+allow-list окружение). Джейлбрейкнутый `rm -rf` / reverse-shell упирается в EROFS/ENETUNREACH/
//! cap-deny на УРОВНЕ ЯДРА песочницы, а не в host-Rust-if с полными правами. Поэтому конструкция
//! `process::Command` для exec-команды агента живёт ТОЛЬКО ЗДЕСЬ — линт `check-sandbox-exec.mjs` валит CI,
//! если она появится в любом host-sandbox-модуле (exec_host/runner/child/…). Единственное исключение —
//! `runner.rs` запускает САМ podman (маркер `sandbox-exec-lint: allow podman-launch`), что и есть запуск
//! песочницы, а не команды агента.
//!
//! Гарантии исполнителя (security-инварианты, пинятся тестами):
//!  - **INV-NO-SHELL**: `Command::new(argv[0]).args(argv[1..])` — НИКОГДА `sh -c`; метасимволы (`;`/`|`/`$()`)
//!    безвредны как argv-байты.
//!  - **INV-ENV-FAILCLOSED**: `env_clear()` ВСЕГДА перед `envs(go.env)` — команда не наследует даже
//!    окружение in-container agentd; видит РОВНО `go.env` (host собрал его из пустого+allow-list, §5.4).
//!  - **INV-CWD-CONFINE**: cwd резолвится ТОЛЬКО под scratch-tmpfs/vault-`:ro` лексическим правилом
//!    [`crate::actuator::classify::path_confinement`] (единый источник, не копия); побег → команда НЕ
//!    запускается. Defense-in-depth поверх kernel-`:ro`.
//!  - **timeout**: wall-clock-кэп (`go.timeout_ms`) → kill + `timed_out=true`. На таймауте хвосты ПУСТЫ,
//!    `*_truncated=false` (частичный вывод намеренно отбрасывается — `timed_out=true` единственный сигнал).
//!    NB: команда, форкнувшая демон-внука, держащего pipe-FD открытым, не даст EOF → exec висит до таймаута
//!    (ограниченная задержка-в-бюджете; внук-сирота пожинается teardown'ом контейнера — live-кейс 6c-3).
//!  - **output-cap**: потоковое чтение stdout/stderr с кольцевым хвостом (`go.output_cap_bytes`) — без
//!    безлимитного `read_to_end` (анти-OOM для «болтливой» команды); за кэпом — `*_truncated=true`.
//!    Ошибка чтения посреди потока трактуется как EOF (fail-soft: хвост может быть короче; реальный сигнал
//!    несут `exit_code`/`timed_out`).
//!
//! 6c-2a: исполнитель + `ExecRunner`-шов + CI-линт. Инертен (вызывающих нет) до 6c-2e (инструменты).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::exec_host::{ExecCwd, WireExecGo};

/// Исход исполнения exec-команды ВНУТРИ песочницы. Кросс-платформенный (вся структура — данные).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    /// Код выхода процесса (`-1`, если завершён сигналом / убит по таймауту; `127` — не удалось запустить).
    pub exit_code: i32,
    /// Хвост stdout (последние ≤`output_cap_bytes` байт, lossy-UTF-8).
    pub stdout_tail: String,
    /// Хвост stderr (последние ≤`output_cap_bytes` байт, lossy-UTF-8).
    pub stderr_tail: String,
    /// stdout превысил кэп (хвост усечён).
    pub stdout_truncated: bool,
    /// stderr превысил кэп (хвост усечён).
    pub stderr_truncated: bool,
    /// Команда убита по wall-clock-таймауту.
    pub timed_out: bool,
}

impl ExecResult {
    /// Не удалось ДАЖЕ запустить процесс (пустой argv / резолв cwd / spawn-ошибка). exit=127 (конвенция
    /// «command not found»), причина в `stderr_tail`. Команда не исполнялась.
    fn launch_failure(reason: impl Into<String>) -> Self {
        Self {
            exit_code: 127,
            stdout_tail: String::new(),
            stderr_tail: reason.into(),
            stdout_truncated: false,
            stderr_truncated: false,
            timed_out: false,
        }
    }
}

/// Шов исполнителя exec-команды. Инструменты (6c-2e) держат `Arc<dyn ExecRunner>` → Tier-1-тесты гоняют
/// [`MockExecRunner`] (без podman, на ЛЮБОМ хосте), прод — [`RealExecRunner`] (единственный реальный
/// `Command`). `scratch_root`/`vault_ro_root` — КОНТЕЙНЕРНЫЕ корни (`/tmp` / `/vault`), резолв-базы cwd.
#[async_trait]
pub trait ExecRunner: Send + Sync {
    async fn run(&self, go: &WireExecGo, scratch_root: &Path, vault_ro_root: &Path) -> ExecResult;
}

/// Прод-исполнитель: ЕДИНСТВЕННАЯ во всём core/host конструкция `process::Command` для команды агента.
/// Бежит ВНУТРИ песочницы (см. module-doc). `unit` — состояния нет.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealExecRunner;

#[async_trait]
impl ExecRunner for RealExecRunner {
    async fn run(&self, go: &WireExecGo, scratch_root: &Path, vault_ro_root: &Path) -> ExecResult {
        let Some(program) = go.argv.first() else {
            return ExecResult::launch_failure("пустой argv — нечего исполнять");
        };
        let cwd = match resolve_cwd(&go.cwd, scratch_root, vault_ro_root) {
            Ok(p) => p,
            Err(e) => return ExecResult::launch_failure(format!("cwd: {e}")),
        };

        // INV-NO-SHELL: программа = argv[0], аргументы = argv[1..]; НИКОГДА `sh -c`.
        // INV-ENV-FAILCLOSED: env_clear() ДО envs(go.env) — наследования нет, видим только go.env.
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(&go.argv[1..])
            .env_clear()
            .envs(go.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return ExecResult::launch_failure(format!("не удалось запустить {program}: {e}"))
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let cap = go.output_cap_bytes;

        // Читаем ОБА потока конкурентно с ожиданием выхода (иначе полный pipe-буфер подвесил бы команду).
        let exec = async {
            let (out, err) =
                tokio::join!(read_capped_tail(stdout, cap), read_capped_tail(stderr, cap));
            let status = child.wait().await;
            (out, err, status)
        };

        let timeout_ms = go.timeout_ms.max(1);
        let timeout = std::time::Duration::from_millis(timeout_ms);
        match tokio::time::timeout(timeout, exec).await {
            Ok(((out_tail, out_trunc), (err_tail, err_trunc), status)) => ExecResult {
                exit_code: status.ok().and_then(|s| s.code()).unwrap_or(-1),
                stdout_tail: out_tail,
                stderr_tail: err_tail,
                stdout_truncated: out_trunc,
                stderr_truncated: err_trunc,
                timed_out: false,
            },
            // Таймаут: `exec` (с заёмом child) уже дропнут → kill доступен. kill_on_drop — страховка.
            Err(_) => {
                let _ = child.kill().await;
                ExecResult {
                    exit_code: -1,
                    stdout_tail: String::new(),
                    stderr_tail: format!("команда убита по таймауту ({timeout_ms} мс)"),
                    stdout_truncated: false,
                    stderr_truncated: false,
                    timed_out: true,
                }
            }
        }
    }
}

/// Потоково читает `reader` до EOF, удерживая ПОСЛЕДНИЕ `cap` байт (кольцо). Возвращает `(tail, truncated)`;
/// `truncated` = всего прочитано > `cap`. Без безлимитного `read_to_end` — анти-OOM. `None`-reader → пусто.
async fn read_capped_tail<R: AsyncRead + Unpin>(reader: Option<R>, cap: usize) -> (String, bool) {
    let Some(mut reader) = reader else {
        return (String::new(), false);
    };
    let mut ring: VecDeque<u8> = VecDeque::new();
    let mut total: usize = 0;
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if cap == 0 {
                    continue; // считаем total для truncated-флага, хвост не храним
                }
                let chunk = &buf[..n];
                if chunk.len() >= cap {
                    // Один чанк перекрывает кэп → хвост = его последние cap байт.
                    ring.clear();
                    ring.extend(&chunk[chunk.len() - cap..]);
                } else {
                    ring.extend(chunk);
                    while ring.len() > cap {
                        ring.pop_front();
                    }
                }
            }
            Err(_) => break,
        }
    }
    let truncated = total > cap;
    let bytes: Vec<u8> = ring.into_iter().collect();
    (String::from_utf8_lossy(&bytes).into_owned(), truncated)
}

/// Резолвит cwd exec-команды ТОЛЬКО под scratch-tmpfs ([`ExecCwd::ScratchTmpfs`]) или vault-`:ro`
/// ([`ExecCwd::VaultRo`]) корнем. `rel` конфайнится ЕДИНЫМ правилом [`path_confinement`] (no `..`/abs/
/// backslash/dot-компонент); пустой/`.` → база. Любой побег → `Err` (команда НЕ запускается). Defense-in-
/// depth поверх kernel-`:ro` (Tier-2).
pub fn resolve_cwd(
    cwd: &ExecCwd,
    scratch_root: &Path,
    vault_ro_root: &Path,
) -> Result<PathBuf, &'static str> {
    let (base, rel) = match cwd {
        ExecCwd::ScratchTmpfs { rel } => (scratch_root, rel.as_str()),
        ExecCwd::VaultRo { rel } => (vault_ro_root, rel.as_str()),
    };
    let rel = rel.trim();
    if rel.is_empty() || rel == "." {
        return Ok(base.to_path_buf());
    }
    crate::actuator::classify::path_confinement(rel)
        .map_err(|_| "cwd_rel вне песочного конфайнмента")?;
    Ok(base.join(rel))
}

/// Тест-исполнитель: захватывает поданный `WireExecGo` (argv/env/cwd-резолв) и отдаёт СКРИПТОВАННЫЙ исход.
/// Линчпин Tier-1-без-podman: security-ассерты (env-содержимое, no-shell, cwd-reject) гоняются на ЛЮБОМ
/// хосте. `pub(crate)` — переиспользуется тестами инструментов (6c-2e).
#[cfg(test)]
pub(crate) struct MockExecRunner {
    /// Последний поданный go (для ассертов argv/env/cwd).
    pub last: std::sync::Mutex<Option<WireExecGo>>,
    /// Резолв cwd, который дал `run` (Ok-путь или текст ошибки) — для cwd-reject-ассертов.
    pub last_cwd: std::sync::Mutex<Option<Result<PathBuf, String>>>,
    /// Скриптованный исход.
    pub result: ExecResult,
}

#[cfg(test)]
impl MockExecRunner {
    pub fn new(result: ExecResult) -> Self {
        Self {
            last: std::sync::Mutex::new(None),
            last_cwd: std::sync::Mutex::new(None),
            result,
        }
    }
}

#[cfg(test)]
#[async_trait]
impl ExecRunner for MockExecRunner {
    async fn run(&self, go: &WireExecGo, scratch_root: &Path, vault_ro_root: &Path) -> ExecResult {
        *self.last.lock().unwrap() = Some(go.clone());
        *self.last_cwd.lock().unwrap() =
            Some(resolve_cwd(&go.cwd, scratch_root, vault_ro_root).map_err(|e| e.to_string()));
        self.result.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(
        argv: Vec<&str>,
        env: Vec<(&str, &str)>,
        cwd: ExecCwd,
        cap: usize,
        timeout_ms: u64,
    ) -> WireExecGo {
        WireExecGo {
            argv: argv.into_iter().map(String::from).collect(),
            cwd,
            env: env
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            timeout_ms,
            output_cap_bytes: cap,
        }
    }

    fn scratch(rel: &str) -> ExecCwd {
        ExecCwd::ScratchTmpfs { rel: rel.into() }
    }

    /// Анти-флейк: wall-clock-кэп для «мгновенных» реальных бинарей (`env`/`echo`/`false`/`head`) —
    /// он ловит ЗАВИСАНИЕ, а не скорость. Прежние 5с мигали (`real_env_clear_proven`, 2× за сутки)
    /// под полной параллельной нагрузкой (`cargo test --workspace` + clippy): fork/exec + pipe + wait
    /// реального процесса под perf-давлением выходили за кэп → ложный `timed_out` → красный ассерт.
    /// 30с на порядки выше честного пути (~0.02с в изоляции) и по-прежнему жёстко ловит регресс-
    /// зависание. НЕ применять к тестам, где таймаут — часть семантики (150/250мс + elapsed-границы).
    #[cfg(unix)]
    const HANG_GUARD_MS: u64 = 30_000;

    /// Первый существующий путь из кандидатов (macOS=/usr/bin/false, Linux=/bin/false и т.п. различаются).
    #[cfg(unix)]
    fn first_existing(cands: &[&'static str]) -> &'static str {
        cands
            .iter()
            .copied()
            .find(|p| Path::new(p).exists())
            .unwrap_or(cands[0])
    }

    // ── resolve_cwd ──────────────────────────────────────────────────────────────────────────────
    #[test]
    fn resolve_cwd_scratch_under_tmpfs() {
        let p = resolve_cwd(&scratch("a/b"), Path::new("/tmp"), Path::new("/vault")).unwrap();
        assert_eq!(p, Path::new("/tmp/a/b"));
    }

    #[test]
    fn resolve_cwd_vaultro_under_vault() {
        let p = resolve_cwd(
            &ExecCwd::VaultRo {
                rel: "notes".into(),
            },
            Path::new("/tmp"),
            Path::new("/vault"),
        )
        .unwrap();
        assert_eq!(p, Path::new("/vault/notes"));
    }

    #[test]
    fn resolve_cwd_empty_or_dot_is_base() {
        assert_eq!(
            resolve_cwd(&scratch(""), Path::new("/tmp"), Path::new("/vault")).unwrap(),
            Path::new("/tmp")
        );
        assert_eq!(
            resolve_cwd(&scratch("."), Path::new("/tmp"), Path::new("/vault")).unwrap(),
            Path::new("/tmp")
        );
    }

    #[test]
    fn resolve_cwd_rejects_escape() {
        for bad in ["../x", "/abs", "a\\..\\x", ".git/x", "a/../../b"] {
            assert!(
                resolve_cwd(&scratch(bad), Path::new("/tmp"), Path::new("/vault")).is_err(),
                "побег должен быть отвергнут: {bad}"
            );
        }
    }

    // ── MockExecRunner: чистая Tier-1-проверка контракта (без процессов) ──────────────────────────
    #[tokio::test]
    async fn mock_captures_go_and_resolves_cwd() {
        let m = MockExecRunner::new(ExecResult {
            exit_code: 0,
            stdout_tail: "ok".into(),
            stderr_tail: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            timed_out: false,
        });
        let g = go(
            vec!["echo", "hi"],
            vec![("PATH", "/bin")],
            scratch("sub"),
            1024,
            1000,
        );
        let r = m.run(&g, Path::new("/tmp"), Path::new("/vault")).await;
        assert_eq!(r.exit_code, 0);
        let captured = m.last.lock().unwrap().clone().unwrap();
        assert_eq!(captured.argv, vec!["echo", "hi"]);
        assert_eq!(captured.env, vec![("PATH".to_string(), "/bin".to_string())]);
        let cwd = m.last_cwd.lock().unwrap().clone().unwrap();
        assert_eq!(cwd, Ok(PathBuf::from("/tmp/sub")));
    }

    // ── read_capped_tail ─────────────────────────────────────────────────────────────────────────
    #[tokio::test]
    async fn capped_tail_keeps_last_bytes_and_flags_truncated() {
        let data = b"0123456789ABCDEF".to_vec(); // 16 байт
        let (tail, trunc) = read_capped_tail(Some(&data[..]), 4).await;
        assert_eq!(tail, "CDEF", "хвост = последние cap байт");
        assert!(trunc, "16 > 4 → truncated");
    }

    #[tokio::test]
    async fn capped_tail_under_cap_not_truncated() {
        let data = b"abc".to_vec();
        let (tail, trunc) = read_capped_tail(Some(&data[..]), 64).await;
        assert_eq!(tail, "abc");
        assert!(!trunc);
    }

    #[tokio::test]
    async fn capped_tail_zero_cap_empty_but_counts() {
        let data = b"abc".to_vec();
        let (tail, trunc) = read_capped_tail(Some(&data[..]), 0).await;
        assert_eq!(tail, "");
        assert!(trunc, "cap=0, есть вывод → truncated");
    }

    // ── RealExecRunner: unix-gated тривиальные бинари (НЕ podman; на CI-хосте) ─────────────────────
    #[cfg(unix)]
    #[tokio::test]
    async fn real_runs_trivial_argv() {
        let echo = first_existing(&["/bin/echo", "/usr/bin/echo"]);
        let r = RealExecRunner
            .run(
                &go(vec![echo, "hi"], vec![], scratch(""), 65536, HANG_GUARD_MS),
                Path::new("/tmp"),
                Path::new("/tmp"),
            )
            .await;
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout_tail.trim_end(), "hi");
        assert!(!r.timed_out);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_nonzero_exit_propagates() {
        let false_bin = first_existing(&["/bin/false", "/usr/bin/false"]);
        let r = RealExecRunner
            .run(
                &go(vec![false_bin], vec![], scratch(""), 1024, HANG_GUARD_MS),
                Path::new("/tmp"),
                Path::new("/tmp"),
            )
            .await;
        assert_eq!(r.exit_code, 1);
    }

    /// INV-ENV-FAILCLOSED доказано на реальном процессе: `env` печатает РОВНО go.env, без host-секрета.
    #[cfg(unix)]
    #[tokio::test]
    async fn real_env_clear_proven() {
        std::env::set_var("NEXUS_FAKE_SECRET", "leaked");
        let env_bin = first_existing(&["/usr/bin/env", "/bin/env"]);
        let r = RealExecRunner
            .run(
                &go(
                    vec![env_bin],
                    vec![("ONLY_VAR", "1")],
                    scratch(""),
                    65536,
                    HANG_GUARD_MS,
                ),
                Path::new("/tmp"),
                Path::new("/tmp"),
            )
            .await;
        std::env::remove_var("NEXUS_FAKE_SECRET");
        assert_eq!(r.exit_code, 0);
        assert!(
            r.stdout_tail.contains("ONLY_VAR=1"),
            "go.env присутствует: {:?}",
            r.stdout_tail
        );
        assert!(
            !r.stdout_tail.contains("NEXUS_FAKE_SECRET"),
            "env_clear: host-секрет НЕ утёк: {:?}",
            r.stdout_tail
        );
    }

    /// ФОРК-ВНУК ДЕРЖИТ ТРУБУ (6c-3c, always-CI): родитель `sh` бэкграундит внука (`sleep`), который
    /// наследует и УДЕРЖИВАЕТ stdout fd1, и СРАЗУ выходит 0. `read_capped_tail(stdout)` не получит EOF
    /// (внук держит трубу), но внешний `tokio::time::timeout` ОБЯЗАН вернуть `run()` ~таймаут (drop(exec) →
    /// kill_on_drop), НЕ виснуть до выхода внука (5с). Это podman-free durable-гарантия (форк-демон-кейс).
    #[cfg(unix)]
    #[tokio::test]
    async fn real_forking_grandchild_holds_pipe_returns_at_timeout() {
        let sh = first_existing(&["/bin/sh", "/usr/bin/sh"]);
        let start = std::time::Instant::now();
        let r = RealExecRunner
            .run(
                // внук `sleep 5` держит fd1; sh выходит мгновенно. timeout=250мс ≪ 5с.
                &go(
                    vec![sh, "-c", "sleep 5 & exit 0"],
                    vec![],
                    scratch(""),
                    1024,
                    250,
                ),
                Path::new("/tmp"),
                Path::new("/tmp"),
            )
            .await;
        assert!(
            r.timed_out,
            "внук держит fd1 → истёк timeout (не зависли на удержанной трубе)"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(4),
            "вернулся ~таймаут, не ждал выхода внука (5с): {:?}",
            start.elapsed()
        );
    }

    /// OUTPUT-CAP НА РЕАЛЬНОМ РАННЕРЕ (6c-3c, always-CI): вывод ≫ cap (200КБ через `head -c /dev/zero`,
    /// портируемо macOS+linux) ⇒ `stdout_truncated=true`, хвост ограничен cap (ring-buffer), exit 0, без
    /// `read_to_end`/OOM. Дополняет unit-тест `read_capped_tail` сквозной проверкой реального процесса.
    #[cfg(unix)]
    #[tokio::test]
    async fn real_large_output_capped() {
        let head = first_existing(&["/usr/bin/head", "/bin/head"]);
        let cap = 65_536usize;
        let r = RealExecRunner
            .run(
                &go(
                    vec![head, "-c", "200000", "/dev/zero"],
                    vec![],
                    scratch(""),
                    cap,
                    HANG_GUARD_MS,
                ),
                Path::new("/tmp"),
                Path::new("/tmp"),
            )
            .await;
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout_truncated, "200КБ > cap ⇒ truncated");
        assert!(
            r.stdout_tail.len() <= cap,
            "хвост ограничен cap (ring): {} > {cap}",
            r.stdout_tail.len()
        );
    }

    /// timeout убивает зависшую команду и помечает timed_out (бюджет ~спим дольше таймаута).
    #[cfg(unix)]
    #[tokio::test]
    async fn real_timeout_kills_and_flags() {
        let start = std::time::Instant::now();
        let sleep_bin = first_existing(&["/bin/sleep", "/usr/bin/sleep"]);
        let r = RealExecRunner
            .run(
                &go(vec![sleep_bin, "10"], vec![], scratch(""), 1024, 150),
                Path::new("/tmp"),
                Path::new("/tmp"),
            )
            .await;
        assert!(r.timed_out, "должен быть timed_out");
        assert_eq!(r.exit_code, -1);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "вернулся около таймаута, не висел"
        );
    }

    /// Несуществующий бинарь → launch_failure (exit 127), команда не исполнялась.
    #[cfg(unix)]
    #[tokio::test]
    async fn real_missing_binary_is_launch_failure() {
        let r = RealExecRunner
            .run(
                &go(
                    vec!["/nonexistent/nexus-no-such-bin"],
                    vec![],
                    scratch(""),
                    1024,
                    HANG_GUARD_MS,
                ),
                Path::new("/tmp"),
                Path::new("/tmp"),
            )
            .await;
        assert_eq!(r.exit_code, 127);
        assert!(!r.timed_out);
    }
}
