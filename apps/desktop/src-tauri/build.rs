use std::process::Command;

/// Захватывает git-версию (ветка + короткий хеш + флаг «грязного» дерева) на этапе компиляции
/// и пробрасывает её в бинарь через `rustc-env`. W-20: статусбар показывает `ветка @ хеш`, чтобы
/// в самом приложении было видно, ЧТО запущено (не только в баннере лаунчера/консоли).
///
/// Работает и в dev (`tauri dev` пересобирает Rust после `git reset --hard` лаунчера), и в
/// бандле. Если git недоступен или это не репозиторий (релиз без `.git`) — значения пустые,
/// команда `app_build_info` вернёт только `version`.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn main() {
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let hash = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    // Грязное дерево: непустой вывод `status --porcelain`. Лаунчер запускаться с правками не даёт,
    // но в dev-правках флаг честно показывает рассинхрон.
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    println!("cargo:rustc-env=NEXUS_GIT_BRANCH={branch}");
    println!("cargo:rustc-env=NEXUS_GIT_HASH={hash}");
    println!(
        "cargo:rustc-env=NEXUS_GIT_DIRTY={}",
        if dirty { "1" } else { "0" }
    );

    // Пересобирать build-скрипт при смене HEAD/ветки. logs/HEAD меняется при любом
    // checkout/reset/commit — надёжный триггер пере-захвата хеша.
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        for f in ["HEAD", "logs/HEAD"] {
            let p = std::path::Path::new(&git_dir).join(f);
            if p.exists() {
                println!("cargo:rerun-if-changed={}", p.display());
            }
        }
    }

    tauri_build::build();
}
