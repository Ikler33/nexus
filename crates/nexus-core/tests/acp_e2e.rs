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

/// Собирает пример `mock_acp_agent` и возвращает ТОЧНЫЙ путь его бинаря из `cargo`-вывода
/// (`compiler-artifact.executable`). Через `--message-format=json` — устойчиво к любому target-dir:
/// `--test acp_e2e` (examples не собраны) И **`cargo llvm-cov`** (свой `target/llvm-cov-target/`, где
/// производный-от-`current_exe` путь не совпал бы → спавн падал, ловлено в CI Coverage-job). Внешний
/// build-lock к моменту ПРОГОНА теста уже отпущен → вложенный `cargo build` безопасен (+0 deps).
fn ensure_mock_built() -> PathBuf {
    let out = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "--example",
            "mock_acp_agent",
            "-p",
            "nexus-core",
            "--message-format=json",
        ])
        .output()
        .expect("запуск cargo build --example");
    assert!(
        out.status.success(),
        "сборка примера mock_acp_agent провалилась: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["reason"] == "compiler-artifact" {
            if let Some(exe) = v["executable"].as_str() {
                if exe.contains("mock_acp_agent") {
                    return PathBuf::from(exe);
                }
            }
        }
    }
    panic!("не нашёл executable mock_acp_agent в выводе cargo build --message-format=json");
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
        // ACP-1b: следующий апдейт — план хода (2 записи, статусы/приоритеты).
        let plan = updates.recv().await.expect("plan update");
        match plan.update {
            SessionUpdate::Plan { entries } => {
                assert_eq!(entries.len(), 2, "план из 2 шагов");
                use nexus_core::agent::connect::acp::schema::AcpPlanStatus;
                assert_eq!(entries[0].status, AcpPlanStatus::InProgress);
            }
            other => panic!("ожидался plan, получено {other:?}"),
        }
        // входящий permission → аппрувим allow_once
        let p = perms.recv().await.expect("permission");
        assert_eq!(p.params.options.len(), 2);
        // ACP-1b: permission несёт ДВА diff'а (мульти-файл A+B).
        let diffs = p
            .params
            .tool_call
            .content
            .as_ref()
            .map(|c| {
                c.iter()
                    .filter(|x| {
                        matches!(
                            x,
                            nexus_core::agent::connect::acp::schema::ToolCallContent::Diff(_)
                        )
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(diffs, 2, "мульти-файловый permission: 2 diff'а");
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
