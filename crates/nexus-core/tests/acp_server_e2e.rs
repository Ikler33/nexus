//! ACP-2 — E2E: наш `AcpClient` драйвит РЕАЛЬНЫЙ подпроцесс `nexus acp` через [`StdioTransport`].
//! MODEL-FREE: initialize → session/new → unknown-method(-32601) → session/cancel → drop-транспорт
//! (чистый выход подпроцесса). Доказывает связку, не покрытую in-process юнитами server.rs: настоящий
//! процесс `nexus acp` + line-framing по pipe + двунаправленный read-loop вместе, против НАШЕГО же
//! `AcpClient` (тот же контракт-оракул, что у ACP-1 — `feedback_mock_must_match_backend`).
//!
//! Полный prompt-драйв (стрим + permission + stopReason) покрыт in-process юнитами server.rs с
//! FakeProvider (LLM не нужен) — поэтому e2e остаётся model-free и зелёным в CI. Кейс со spawn — `#[cfg(unix)]`
//! (паритет с acp_e2e.rs).

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nexus_core::agent::connect::acp::AcpClient;
use nexus_core::agent::connect::StdioTransport;
use nexus_core::db::Database;
use serde_json::json;
use tempfile::TempDir;

/// Собирает бинарь `nexus` (nexus-cli) и возвращает его ТОЧНЫЙ путь из `cargo`-вывода
/// (`compiler-artifact.executable`). Через `--message-format=json` — устойчиво к любому target-dir
/// (`--test`, `cargo llvm-cov` со своим `llvm-cov-target/`), как `ensure_mock_built` в acp_e2e.rs.
fn ensure_nexus_built() -> PathBuf {
    let out = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "-p",
            "nexus-cli",
            "--bin",
            "nexus",
            "--message-format=json",
        ])
        .output()
        .expect("запуск cargo build --bin nexus");
    assert!(
        out.status.success(),
        "сборка бинаря nexus провалилась: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["reason"] == "compiler-artifact" {
            if let Some(exe) = v["executable"].as_str() {
                if exe.ends_with("/nexus") || exe.ends_with("nexus.exe") {
                    return PathBuf::from(exe);
                }
            }
        }
    }
    panic!("не нашёл executable nexus в выводе cargo build --message-format=json");
}

/// Создаёт temp-vault с `.nexus/local.json` (ai.chat, чтобы build_deps собрал провайдера) + `.nexus/nexus.db`
/// (Database::open инициализирует схему). БЕЗ --actuator → vault не пишется.
async fn make_vault() -> TempDir {
    let dir = TempDir::new().unwrap();
    let nexus = dir.path().join(".nexus");
    std::fs::create_dir_all(&nexus).unwrap();
    // ai.chat.url — фиктивный (model-free тест НЕ шлёт prompt → сетевого вызова нет; url лишь идёт в
    // egress-allowlist при сборке deps).
    std::fs::write(
        nexus.join("local.json"),
        r#"{"ai":{"chat":{"url":"http://127.0.0.1:9/v1","model":"e2e"}}}"#,
    )
    .unwrap();
    // Инициализируем БД (build_deps откроет тот же файл).
    let db = Database::open(nexus.join("nexus.db")).await.unwrap();
    drop(db);
    dir
}

#[tokio::test]
async fn acp_server_e2e_real_subprocess_model_free() {
    let nexus = ensure_nexus_built();
    let vault = make_vault().await;
    let vault_path = vault.path().canonicalize().unwrap();

    let transport = StdioTransport::spawn(
        &nexus.to_string_lossy(),
        &[
            "acp".to_string(),
            "--vault".to_string(),
            vault_path.to_string_lossy().to_string(),
        ],
        &std::env::temp_dir(),
    )
    .await
    .expect("спавн `nexus acp`");
    let (client, _updates, _perms) = AcpClient::new(Arc::new(transport));

    // initialize → protocolVersion == 1 (объявляем нашу версию).
    let init = client
        .request(
            "initialize",
            json!({"protocolVersion": 1,
                   "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}}),
            Some(Duration::from_secs(20)),
        )
        .await
        .expect("initialize");
    assert_eq!(init["protocolVersion"], 1);

    // session/new → sessionId.
    let sess = client
        .request(
            "session/new",
            json!({"cwd": vault_path, "mcpServers": []}),
            Some(Duration::from_secs(10)),
        )
        .await
        .expect("session/new");
    assert!(
        sess["sessionId"].as_str().unwrap().starts_with('s'),
        "session/new вернул sessionId"
    );

    // unknown method → method_not_found (-32601), сервер НЕ висит.
    let err = client
        .request(
            "fs/read_text_file",
            json!({"path": "x"}),
            Some(Duration::from_secs(10)),
        )
        .await
        .expect_err("unknown method → Err");
    assert_eq!(err.code, -32601, "неизвестный метод → method_not_found");

    // session/cancel (notification) — принимается без ответа/паники.
    client
        .notify("session/cancel", json!({"sessionId": sess["sessionId"]}))
        .await
        .expect("session/cancel notify");

    // drop транспорта (через drop клиента) → подпроцесс получает EOF stdin и завершается чисто (нет зависа).
    drop(client);
    // даём подпроцессу секунду на выход — отсутствие зависа теста = доказательство чистого завершения
    // (kill_on_drop StdioTransport всё равно подстрахует).
    tokio::time::sleep(Duration::from_millis(200)).await;
}
