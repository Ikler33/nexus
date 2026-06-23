//! exec_it — Tier-2 интеграционные тесты exec-песочницы (SANDBOX-6c-3, спека §8), требующие РЕАЛЬНОГО
//! Podman. Test-only модуль (`#[cfg(test)] mod exec_it`).
//!
//! Tier-1 (без podman, на любом хосте/в CI) доказал контракт через `MockExecRunner`; Tier-2 доказывает,
//! что РЕАЛЬНЫЙ `--network=none` контейнер ENFORCE'ит то, что Tier-1 мокал (EROFS/ENETUNREACH/env/argv —
//! слайсы 6c-3b/c; killed-container reaper — 6c-3f live). Podman НЕТ ни локально (macOS), ни в CI → эти
//! тесты помечены `ignore` и гоняются ТОЛЬКО на .28 (см. docs/runbooks/sandbox-tier2.md, 6c-3d).
//!
//! # Тройной fail-closed lock ([`podman_it_enabled`]) — INV-GATE-SINGLE / INV-GATE-FAILCLOSED
//! Podman-`ignore`-тест исполняется ТОЛЬКО когда ВСЕ три условия истинны: (1) `cfg(target_os="linux")`
//! на самом тесте; (2) атрибут `ignore` (обычный CI его пропускает); (3) `podman_it_enabled()` == оператор
//! выставил `NEXUS_SANDBOX_IT=1` И реальный `podman --version` вернул exit 0. Случайный
//! `cargo test -- --ignored` на podman-less Linux без env-переменной → ранний `return` (no-op, НЕ
//! false-red). ЕДИНЫЙ предикат на ВСЕ Tier-2 тесты — нет разрозненных env-чтений (каждый новый Tier-2
//! слайс зовёт [`podman_it_enabled`], а не свой `std::env::var`).

/// Чистый комбинатор гейта — тестируется БЕЗ реального бинаря/env. Оба условия ОБЯЗАТЕЛЬНЫ (fail-closed).
pub(crate) fn it_gate(env_set: bool, podman_present: bool) -> bool {
    env_set && podman_present
}

/// Выставил ли оператор `NEXUS_SANDBOX_IT=1` (.28-runbook). Иначе Tier-2 off (CI/дев — всегда off).
fn it_env_set() -> bool {
    std::env::var("NEXUS_SANDBOX_IT").as_deref() == Ok("1")
}

/// Реальный podman доступен (best-effort probe). Вне Linux — ВСЕГДА false: keep-id/SO_PEERCRED/EROFS —
/// linux-семантика, не пытаемся даже на podman-desktop под macOS.
#[cfg(target_os = "linux")]
fn podman_present() -> bool {
    // sandbox-exec-lint: allow podman-probe (диагностика рантайма Tier-2, НЕ exec команды агента —
    // host тут лишь проверяет наличие podman, не спавнит команду модели мимо песочницы).
    std::process::Command::new("podman")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn podman_present() -> bool {
    false
}

/// ЕДИНЫЙ gate для ВСЕХ Tier-2 podman-тестов (6c-3b/c/e/f). Fail-closed: env=1 И реальный podman.
pub(crate) fn podman_it_enabled() -> bool {
    it_gate(it_env_set(), podman_present())
}

/// БЕЗОПАСЕН ли путь тест-vault: НЕ под `$HOME/.nexus` (живой agentd-vault). Tier-2 ОБЯЗАН использовать
/// выделенный TempDir/тест-vault, НИКОГДА прод. Структурный guard — лучше упасть тест, чем тронуть прод.
pub(crate) fn is_safe_test_vault(path: &std::path::Path) -> bool {
    match std::env::var_os("HOME") {
        Some(home) => !path.starts_with(std::path::Path::new(&home).join(".nexus")),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gate fail-closed: ни env-без-podman, ни podman-без-env не включают Tier-2 — нужны ОБА.
    #[test]
    fn podman_it_requires_both_env_and_binary() {
        assert!(!it_gate(false, false));
        assert!(!it_gate(true, false), "env без podman → off");
        assert!(!it_gate(false, true), "podman без env → off");
        assert!(it_gate(true, true), "оба → on");
    }

    /// В CI/деве (NEXUS_SANDBOX_IT не выставлен) предикат false — нулевые podman-вызовы, никаких false-red.
    #[test]
    fn podman_it_disabled_without_env() {
        if std::env::var("NEXUS_SANDBOX_IT").is_err() {
            assert!(!podman_it_enabled(), "без env Tier-2 выключен");
        }
    }

    /// Тест-vault guard отвергает живой ~/.nexus, пропускает tmp.
    #[test]
    fn test_vault_guard_refuses_home_nexus() {
        if let Some(home) = std::env::var_os("HOME") {
            let live = std::path::Path::new(&home).join(".nexus").join("vault");
            assert!(!is_safe_test_vault(&live), "живой ~/.nexus/vault отвергнут");
        }
        assert!(
            is_safe_test_vault(std::path::Path::new("/tmp/sbx-test-vault")),
            "tmp-vault безопасен"
        );
    }
}

/// Podman-зависимые `ignore`-тесты — ТОЛЬКО Linux + явный gate. 6c-3a кладёт драйвер-смоук; матрицу
/// containment (EROFS/ENETUNREACH/env/argv) добавят 6c-3b/c, killed-container — 6c-3f live.
#[cfg(target_os = "linux")]
mod tier2 {
    use super::*;
    use crate::sandbox::DEFAULT_SANDBOX_IMAGE;

    /// Драйвер-смоук: `podman run --rm --network=none <image> true` → exit 0. Доказывает podman+образ+gate
    /// прежде любых containment-ассертов. No-op без `NEXUS_SANDBOX_IT=1` + реального podman.
    #[test]
    #[ignore = "Tier-2: требует Podman на .28 (NEXUS_SANDBOX_IT=1)"]
    fn podman_smoke_runs_trivial_container() {
        if !podman_it_enabled() {
            return; // fail-closed no-op (нет podman/env)
        }
        // sandbox-exec-lint: allow podman-launch (запуск САМОГО podman для Tier-2-смоука, не exec агента).
        let status = std::process::Command::new("podman")
            .args([
                "run",
                "--rm",
                "--network=none",
                // образ несёт ENTRYPOINT=nexus-agentd → очищаем, иначе `true` уедет ему в argv
                // как vault-путь (`nexus-agentd true` → «vault path true: No such file»).
                "--entrypoint",
                "",
                DEFAULT_SANDBOX_IMAGE,
                "true",
            ])
            .status()
            .expect("podman run");
        assert!(
            status.success(),
            "podman run --network=none <image> true → exit 0"
        );
    }

    /// Запуск команды в ХАРДНЕННОМ podman-контейнере (зеркало sandbox_run_plan-флагов: `--network=none`
    /// `--read-only` `--tmpfs /tmp` `--cap-drop=ALL` `--security-opt=no-new-privileges`) + доп. аргументы.
    /// Возвращает `(exit, stdout, stderr)`. Только для Tier-2-тестов (gated podman_it_enabled).
    fn hardened_podman_run(extra: &[&str], cmd: &[&str]) -> (i32, String, String) {
        let mut a: Vec<String> = [
            "run",
            "--rm",
            "--network=none",
            "--read-only",
            "--tmpfs",
            "/tmp",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            // образ несёт ENTRYPOINT=nexus-agentd → очищаем, чтобы прогонять сырые probe-команды
            // (`cat`/`touch`/…), а не отдавать их агент-бинарю как vault-путь.
            "--entrypoint",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        a.extend(extra.iter().map(|s| s.to_string()));
        a.push(DEFAULT_SANDBOX_IMAGE.to_string());
        a.extend(cmd.iter().map(|s| s.to_string()));
        // sandbox-exec-lint: allow podman-launch (Tier-2 контейнерный containment-прогон, не exec агента).
        let out = std::process::Command::new("podman")
            .args(&a)
            .output()
            .expect("podman run");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// KERNEL-EROFS: запись в `:ro`-vault внутри контейнера падает (Read-only file system), файл не создан.
    /// Доказывает kernel-enforcement vault-`:ro` (Tier-1 мокал; Tier-2 — реальный mount).
    #[test]
    #[ignore = "Tier-2: требует Podman на .28 (NEXUS_SANDBOX_IT=1)"]
    fn real_vault_write_is_erofs() {
        if !podman_it_enabled() {
            return;
        }
        let vault = tempfile::TempDir::new().expect("tempdir");
        assert!(
            is_safe_test_vault(vault.path()),
            "тест-vault не под ~/.nexus"
        );
        let mount = format!("{}:/vault:ro", vault.path().display());
        // `--user 0:0`: контейнер-root маппится (rootless userns) на host-владельца mount'а, поэтому
        // ЕДИНСТВЕННЫЙ блокиратор записи — флаг `:ro` → чистая EROFS-сигнатура. Под дефолтным USER nexus
        // (uid 10001, не владелец) запись падает РАНЬШЕ по EACCES (ownership), маскируя EROFS.
        let (code, _out, err) =
            hardened_podman_run(&["-v", &mount, "--user", "0:0"], &["touch", "/vault/probe"]);
        assert_ne!(code, 0, "запись в :ro vault должна падать (EROFS)");
        assert!(
            err.to_lowercase().contains("read-only"),
            "EROFS-сигнатура в stderr: {err:?}"
        );
        assert!(
            !vault.path().join("probe").exists(),
            "файл НЕ создан на host-стороне"
        );
    }

    /// KERNEL-ENETUNREACH: внутри `--network=none` нет маршрутов (`/proc/net/route` — только заголовок).
    /// Доказывает реальную сетевую изоляцию exec (Tier-1 не мог — нужен kernel namespace).
    #[test]
    #[ignore = "Tier-2: требует Podman на .28 (NEXUS_SANDBOX_IT=1)"]
    fn no_network_route_inside_exec() {
        if !podman_it_enabled() {
            return;
        }
        let (code, out, _err) = hardened_podman_run(&[], &["cat", "/proc/net/route"]);
        assert_eq!(code, 0, "cat /proc/net/route → exit 0");
        let data_lines = out
            .lines()
            .skip(1) // строка-заголовок Iface/Destination/...
            .filter(|l| !l.trim().is_empty())
            .count();
        assert_eq!(
            data_lines, 0,
            "в --network=none нет сетевых маршрутов: {out:?}"
        );
    }
}
