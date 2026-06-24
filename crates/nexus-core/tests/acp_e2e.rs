//! ACP-1 — E2E: спавним РЕАЛЬНЫЙ подпроцесс-агент (`examples/mock_acp_agent`) через [`StdioTransport`] и
//! драйвим его [`AcpClient`]'ом по полному пути initialize → session/new → session/prompt (стрим +
//! request_permission round-trip → end_turn). Это единственный путь, не покрытый юнитами: настоящий
//! процесс + line-framing по pipe + bidirectional read-loop ВМЕСТЕ (юниты client.rs гоняют протокол по
//! in-process ChannelTransport, юниты stdio.rs — framing через `cat`; здесь — всё сразу против чужого
//! бинаря). Мок — НЕзависимая реализация контракта → ловит дрейф схемы (`feedback_mock_must_match_backend`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nexus_core::agent::connect::acp::schema::SessionUpdate;
use nexus_core::agent::connect::acp::AcpClient;
use nexus_core::agent::connect::StdioTransport;
use serde_json::json;

/// Путь к собранному примеру `mock_acp_agent` (`target/<profile>/examples/mock_acp_agent[.exe]`).
fn mock_agent_path() -> PathBuf {
    let mut dir = std::env::current_exe().expect("current_exe");
    dir.pop(); // имя тест-бинаря
    if dir.ends_with("deps") {
        dir.pop(); // deps/ → target/<profile>/
    }
    let exe = if cfg!(windows) {
        "mock_acp_agent.exe"
    } else {
        "mock_acp_agent"
    };
    dir.join("examples").join(exe)
}

/// Гарантирует, что пример собран. Полный `cargo test` собирает examples сам, но `--test acp_e2e` — нет;
/// досборка on-demand делает тест устойчивым к любому способу запуска. Внешний build-lock к моменту
/// ПРОГОНА теста уже отпущен (тесты идут после сборки) → вложенный `cargo build` безопасен (+0 deps).
fn ensure_mock_built() -> PathBuf {
    let path = mock_agent_path();
    if path.exists() {
        return path;
    }
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "--example", "mock_acp_agent", "-p", "nexus-core"])
        .status()
        .expect("запуск cargo build --example");
    assert!(
        status.success(),
        "сборка примера mock_acp_agent провалилась"
    );
    path
}

#[tokio::test]
async fn acp_e2e_real_subprocess_full_run() {
    let agent = ensure_mock_built();
    let cwd = std::env::temp_dir();
    let transport = StdioTransport::spawn(&agent.to_string_lossy(), &[], &cwd)
        .await
        .expect("спавн mock-агента");
    let (client, mut updates, mut perms) = AcpClient::new(Arc::new(transport));

    // initialize (с таймаутом — управляющий RPC)
    let init = client
        .request(
            "initialize",
            json!({"protocolVersion":1,
                   "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}),
            Some(Duration::from_secs(10)),
        )
        .await
        .expect("initialize");
    assert_eq!(init["protocolVersion"], 1);

    // session/new
    let sess = client
        .request(
            "session/new",
            json!({"cwd": cwd, "mcpServers": []}),
            Some(Duration::from_secs(10)),
        )
        .await
        .expect("session/new");
    assert_eq!(sess["sessionId"], "s1");

    // session/prompt (БЕЗ таймаута) — конкурентно с обработкой стрима/permission
    let prompt = client.request(
        "session/prompt",
        json!({"sessionId":"s1","prompt":[{"type":"text","text":"do it"}]}),
        None,
    );

    let drive = async {
        // первый апдейт — токен ассистента
        let first = updates.recv().await.expect("первый update");
        assert!(matches!(
            first.update,
            SessionUpdate::AgentMessageChunk { .. }
        ));
        // входящий permission → аппрувим allow_once
        let p = perms.recv().await.expect("permission");
        assert_eq!(p.params.options.len(), 2);
        let allow = p
            .params
            .options
            .iter()
            .find(|o| o.option_id == "a")
            .expect("allow-опция");
        client
            .respond(
                p.id.clone(),
                Ok(json!({"outcome":{"outcome":"selected","optionId":allow.option_id}})),
            )
            .await
            .expect("respond permission");
    };

    let (prompt_res, ()) = tokio::join!(prompt, drive);
    assert_eq!(
        prompt_res.expect("prompt result")["stopReason"],
        "end_turn",
        "после аппрува агент должен завершить ход end_turn"
    );
}
