use super::*;
use crate::actuator::ProposalItem;
use crate::agent::event::{PlanStep, PlanStepState};
use crate::agent::tool::{ToolCall, ToolSpec};
use crate::ai::tools::ToolTurn;
use crate::ai::AiResult;
use crate::db::Database;
use crate::net::RunCtx;
use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use tempfile::TempDir;

use super::super::super::{channel_pair, ChannelTransport};

// ── провайдеры (offline) ──

/// Скриптованный fake: FIFO заданных ходов.
struct FakeProvider {
    turns: StdMutex<VecDeque<AiResult<ToolTurn>>>,
}
impl FakeProvider {
    fn new(turns: Vec<AiResult<ToolTurn>>) -> Self {
        Self {
            turns: StdMutex::new(turns.into_iter().collect()),
        }
    }
}
#[async_trait]
impl ToolCapableProvider for FakeProvider {
    async fn stream_chat_tools(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ToolSpec],
        on_token: &mut (dyn FnMut(String) + Send),
        _cancel: &Arc<AtomicBool>,
        _ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        let next = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(ToolTurn::Final("(no more turns)".into())));
        // На Final эмитим хотя бы один токен (доказ. потока agent_message_chunk).
        if let Ok(ToolTurn::Final(s)) = &next {
            on_token(s.clone());
        }
        next
    }
    fn model_id(&self) -> &str {
        "fake"
    }
}

/// Провайдер, висящий на первом ходу — держит ход активным детерминированно (R2-тест).
struct SleepyProvider;
#[async_trait]
impl ToolCapableProvider for SleepyProvider {
    async fn stream_chat_tools(
        &self,
        _m: &[ChatMessage],
        _t: &[ToolSpec],
        _o: &mut (dyn FnMut(String) + Send),
        _c: &Arc<AtomicBool>,
        _ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        tokio::time::sleep(Duration::from_millis(250)).await;
        Ok(ToolTurn::Final("done".into()))
    }
    fn model_id(&self) -> &str {
        "sleepy"
    }
}

/// Провайдер, который КРУТИТСЯ (каждый ход возвращает ToolCalls со sleep) — чтобы цикл проверял
/// `cancel` на границе шага и останавливался Cancelled (Final никогда не достигается сам).
struct LoopingSleepyProvider;
#[async_trait]
impl ToolCapableProvider for LoopingSleepyProvider {
    async fn stream_chat_tools(
        &self,
        _m: &[ChatMessage],
        _t: &[ToolSpec],
        _o: &mut (dyn FnMut(String) + Send),
        _c: &Arc<AtomicBool>,
        _ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        tokio::time::sleep(Duration::from_millis(80)).await;
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "loop".into(),
            name: "noop".into(),
            arguments: "{}".into(),
        }]))
    }
    fn model_id(&self) -> &str {
        "looping"
    }
}

/// Провайдер, ЗАПИСЫВАЮЩИЙ полученные messages КАЖДОГО хода (мультитёрн-история).
struct RecordingProvider {
    seen: Arc<StdMutex<Vec<Vec<ChatMessage>>>>,
}
#[async_trait]
impl ToolCapableProvider for RecordingProvider {
    async fn stream_chat_tools(
        &self,
        messages: &[ChatMessage],
        _t: &[ToolSpec],
        _o: &mut (dyn FnMut(String) + Send),
        _c: &Arc<AtomicBool>,
        _ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        self.seen.lock().unwrap().push(messages.to_vec());
        Ok(ToolTurn::Final("ok".into()))
    }
    fn model_id(&self) -> &str {
        "rec"
    }
}

// ── харнесс ──

async fn open_db() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path().join("test.db")).await.unwrap();
    (dir, db)
}

fn cfg_with(
    provider: Arc<dyn ToolCapableProvider>,
    canon_root: PathBuf,
    db: &Database,
    actuator_enabled: bool,
    autonomy: &str,
) -> Arc<AcpServerConfig> {
    Arc::new(AcpServerConfig {
        provider,
        writer: db.writer().clone(),
        reader: db.reader().clone(),
        canon_root,
        actuator_enabled,
        autonomy: autonomy.to_string(),
        overwrite_threshold: 64 * 1024,
        blast_cap: 16,
        context_window: Some(8192),
        model: "fake".into(),
    })
}

/// Поднимает serve_acp над server-эндпоинтом, возвращает client-эндпоинт.
fn serve(
    cfg: Arc<AcpServerConfig>,
    client: ChannelTransport,
    server: ChannelTransport,
) -> Arc<ChannelTransport> {
    let server = Arc::new(server);
    tokio::spawn(serve_acp(server, cfg));
    Arc::new(client)
}

/// Достаёт params из Notification (для пуро-функциональных тестов маппинга).
fn notif_params(m: &RpcMessage) -> Value {
    match m {
        RpcMessage::Notification { params, .. } => params.clone(),
        _ => panic!("ожидалась Notification, получено {m:?}"),
    }
}

async fn recv_to(t: &dyn Transport) -> RpcMessage {
    tokio::time::timeout(Duration::from_secs(5), t.recv())
        .await
        .expect("recv timeout")
        .expect("transport closed")
}

/// Шлёт request, ждёт Response с тем же id (пропуская промежуточные notification/прочие Response).
async fn request(
    client: &dyn Transport,
    id: i64,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    client
        .send(RpcMessage::request(id, method, params))
        .await
        .unwrap();
    loop {
        if let RpcMessage::Response { id: rid, result } = recv_to(client).await {
            if rid == json!(id) {
                return result;
            }
        }
    }
}

async fn init_and_session(client: &dyn Transport) -> String {
    let r = request(
        client,
        1,
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    )
    .await
    .unwrap();
    assert_eq!(r["protocolVersion"], 1);
    let r = request(
        client,
        2,
        "session/new",
        json!({"cwd": "/ignored", "mcpServers": []}),
    )
    .await
    .unwrap();
    r["sessionId"].as_str().unwrap().to_string()
}

// ── 1. initialize ──
#[tokio::test]
async fn initialize_returns_protocol_version_1() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(FakeProvider::new(vec![])),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let client = serve(cfg, c, s);
    let r = request(
        client.as_ref(),
        1,
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    )
    .await
    .unwrap();
    assert_eq!(r["protocolVersion"], 1);
}

#[tokio::test]
async fn initialize_non_object_params_invalid() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(FakeProvider::new(vec![])),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let client = serve(cfg, c, s);
    let r = request(client.as_ref(), 1, "initialize", json!("not-an-object")).await;
    assert_eq!(r.unwrap_err().code, -32602);
}

// ── 2. session/new ──
#[tokio::test]
async fn session_new_returns_session_id() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(FakeProvider::new(vec![])),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let client = serve(cfg, c, s);
    let _ = request(
        client.as_ref(),
        1,
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    )
    .await;
    let r = request(
        client.as_ref(),
        2,
        "session/new",
        json!({"cwd": "/x", "mcpServers": []}),
    )
    .await
    .unwrap();
    assert!(r["sessionId"].as_str().unwrap().starts_with('s'));
}

// ── 3. prompt стримит и финалит end_turn ──
#[tokio::test]
async fn prompt_streams_and_finals_end_turn() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: "echo".into(),
            arguments: r#"{"text":"hi"}"#.into(),
        }])),
        Ok(ToolTurn::Final("готово".into())),
    ]));
    let cfg = cfg_with(provider, _d.path().to_path_buf(), &db, false, "confirm");
    let client = serve(cfg, c, s);
    let sid = init_and_session(client.as_ref()).await;

    client
        .send(RpcMessage::request(
            3,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"do"}]}),
        ))
        .await
        .unwrap();

    let mut saw_chunk = false;
    let mut saw_tool_call = false;
    let mut saw_tool_update = false;
    let mut stop = String::new();
    for _ in 0..50 {
        match recv_to(client.as_ref()).await {
            RpcMessage::Notification { method, params } if method == "session/update" => {
                match params["update"]["sessionUpdate"].as_str().unwrap_or("") {
                    "agent_message_chunk" => saw_chunk = true,
                    "tool_call" => saw_tool_call = true,
                    "tool_call_update" => saw_tool_update = true,
                    _ => {}
                }
            }
            RpcMessage::Response { id, result } if id == json!(3) => {
                stop = result.unwrap()["stopReason"].as_str().unwrap().to_string();
                break;
            }
            _ => {}
        }
    }
    assert!(saw_tool_call, "tool_call застримлен");
    assert!(saw_tool_update, "tool_call_update застримлен");
    assert!(saw_chunk, "agent_message_chunk застримлен");
    assert_eq!(stop, "end_turn", "Response пришёл ПОСЛЕ стрима");
}

// ── keystone-permission: helper, гоняющий note.create через гейт с заданным client-исходом ──
async fn run_permission_case(
    autonomy: &str,
    // None → не отвечаем (для transport-close); Some(outcome) → шлём этот /result.
    client_outcome: Option<Value>,
    drop_after_perm: bool,
) -> (bool, String) {
    let (c, s) = channel_pair();
    let (dir, db) = open_db().await;
    let canon = dir.path().canonicalize().unwrap();
    let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "n1".into(),
            name: "note.create".into(),
            arguments: r#"{"path":"Notes/W.md","content":"данные"}"#.into(),
        }])),
        Ok(ToolTurn::Final("готово".into())),
    ]));
    let cfg = cfg_with(provider, canon.clone(), &db, true, autonomy);
    let client = serve(cfg, c, s);
    let sid = init_and_session(client.as_ref()).await;
    client
        .send(RpcMessage::request(
            3,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"создай"}]}),
        ))
        .await
        .unwrap();

    let mut stop = String::new();
    let mut perm_seen = false;
    for _ in 0..80 {
        match recv_to(client.as_ref()).await {
            RpcMessage::Request { id, method, .. } if method == "session/request_permission" => {
                perm_seen = true;
                if drop_after_perm {
                    drop(client); // транспорт закрыт мид-permission → fail-closed
                                  // ждём, пока серверный прогон завершится (он зафиналит)
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    break;
                }
                if let Some(out) = &client_outcome {
                    client
                        .send(RpcMessage::Response {
                            id,
                            result: Ok(out.clone()),
                        })
                        .await
                        .unwrap();
                }
            }
            RpcMessage::Response { id, result } if id == json!(3) => {
                stop = result.unwrap()["stopReason"].as_str().unwrap().to_string();
                break;
            }
            _ => {}
        }
    }
    assert!(
        perm_seen || autonomy == "auto",
        "request_permission ожидался (confirm)"
    );
    let written = std::fs::read_to_string(canon.join("Notes/W.md")).is_ok();
    (written, stop)
}

// ── 4. allow применяет запись ──
#[tokio::test]
async fn permission_allow_applies_write() {
    let (written, stop) = run_permission_case(
        "confirm",
        Some(json!({"outcome": {"outcome": "selected", "optionId": "allow"}})),
        false,
    )
    .await;
    assert!(written, "allow → файл записан через гейт");
    assert_eq!(stop, "end_turn");
}

// ── 5. reject не пишет ──
#[tokio::test]
async fn permission_reject_does_not_write() {
    let (written, stop) = run_permission_case(
        "confirm",
        Some(json!({"outcome": {"outcome": "selected", "optionId": "reject"}})),
        false,
    )
    .await;
    assert!(!written, "reject → файл НЕ записан");
    assert_eq!(stop, "end_turn", "ход всё равно финалит");
}

// ── 6. cancelled не пишет ──
#[tokio::test]
async fn permission_cancelled_does_not_write() {
    let (written, _stop) = run_permission_case(
        "confirm",
        Some(json!({"outcome": {"outcome": "cancelled"}})),
        false,
    )
    .await;
    assert!(!written, "cancelled → reject_all → файл НЕ записан");
}

// ── 7. неизвестная опция не пишет ──
#[tokio::test]
async fn permission_unknown_option_rejects() {
    let (written, _stop) = run_permission_case(
        "confirm",
        Some(json!({"outcome": {"outcome": "selected", "optionId": "bogus"}})),
        false,
    )
    .await;
    assert!(
        !written,
        "неизвестная optionId → reject_all → файл НЕ записан"
    );
}

// ── 8. закрытие транспорта мид-permission → reject_all, без зависа ──
#[tokio::test]
async fn permission_transport_close_rejects() {
    let (written, _stop) = run_permission_case("confirm", None, true).await;
    assert!(
        !written,
        "EOF мид-permission → reject_all (fail-closed), файл НЕ записан"
    );
}

// ── 9. unknown method → -32601 ──
#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(FakeProvider::new(vec![])),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let client = serve(cfg, c, s);
    let r = request(client.as_ref(), 1, "fs/read_text_file", json!({})).await;
    assert_eq!(r.unwrap_err().code, -32601);
}

// ── 10. битые params prompt → -32602 ──
#[tokio::test]
async fn malformed_params_invalid_params() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(FakeProvider::new(vec![])),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let client = serve(cfg, c, s);
    let _ = init_and_session(client.as_ref()).await;
    let r = request(client.as_ref(), 3, "session/prompt", json!({"wrong": 1})).await;
    assert_eq!(r.unwrap_err().code, -32602);
}

// ── 11. вторая session/new → -32602 (R1) ──
#[tokio::test]
async fn second_session_rejected() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(FakeProvider::new(vec![])),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let client = serve(cfg, c, s);
    let _ = init_and_session(client.as_ref()).await;
    let r = request(
        client.as_ref(),
        9,
        "session/new",
        json!({"cwd": "/y", "mcpServers": []}),
    )
    .await;
    assert_eq!(r.unwrap_err().code, -32602, "R1: вторая сессия отклонена");
}

// ── 12. второй prompt при активном → -32602 (R2), затем третий проходит ──
#[tokio::test]
async fn second_prompt_while_active_rejected() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(SleepyProvider),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let client = serve(cfg, c, s);
    let sid = init_and_session(client.as_ref()).await;

    // первый prompt — НЕ ждём ответа (sleepy висит).
    client
        .send(RpcMessage::request(
            3,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"first"}]}),
        ))
        .await
        .unwrap();
    // дать первому взвести active.
    tokio::time::sleep(Duration::from_millis(50)).await;
    // второй prompt при активном → invalid_params (id=4).
    let r = request(
        client.as_ref(),
        4,
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type":"text","text":"second"}]}),
    )
    .await;
    assert_eq!(
        r.unwrap_err().code,
        -32602,
        "R2: второй активный prompt отклонён"
    );

    // дождаться завершения первого (Response id=3) — потом третий проходит.
    let mut first_done = false;
    for _ in 0..80 {
        if let RpcMessage::Response { id, .. } = recv_to(client.as_ref()).await {
            if id == json!(3) {
                first_done = true;
                break;
            }
        }
    }
    assert!(first_done, "первый ход завершился");
    let r3 = request(
        client.as_ref(),
        5,
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type":"text","text":"third"}]}),
    )
    .await;
    assert_eq!(
        r3.unwrap()["stopReason"],
        "end_turn",
        "после завершения — третий ход принят"
    );
}

// ── 13. мультитёрн: ход 2 видит историю хода 1 ──
#[tokio::test]
async fn multi_turn_history_accumulates() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let seen = Arc::new(StdMutex::new(Vec::<Vec<ChatMessage>>::new()));
    let provider: Arc<dyn ToolCapableProvider> = Arc::new(RecordingProvider { seen: seen.clone() });
    let cfg = cfg_with(provider, _d.path().to_path_buf(), &db, false, "confirm");
    let client = serve(cfg, c, s);
    let sid = init_and_session(client.as_ref()).await;

    let r1 = request(
        client.as_ref(),
        3,
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type":"text","text":"ALPHA"}]}),
    )
    .await;
    assert_eq!(r1.unwrap()["stopReason"], "end_turn");
    let r2 = request(
        client.as_ref(),
        4,
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type":"text","text":"BETA"}]}),
    )
    .await;
    assert_eq!(r2.unwrap()["stopReason"], "end_turn");

    let captured = seen.lock().unwrap();
    assert_eq!(captured.len(), 2, "два хода");
    // ход 2 ДОЛЖЕН видеть user(ALPHA)+assistant(ok) из хода 1.
    let turn2_has_alpha = captured[1].iter().any(|m| m.content.contains("ALPHA"));
    let turn2_has_assistant = captured[1].iter().any(|m| m.content == "ok");
    assert!(
        turn2_has_alpha,
        "ход 2 видит user-задачу хода 1 (W-4 история)"
    );
    assert!(turn2_has_assistant, "ход 2 видит assistant-ответ хода 1");
    // ход 1 НЕ должен видеть BETA (порядок).
    assert!(!captured[0].iter().any(|m| m.content.contains("BETA")));
}

// ── 14. session/cancel взводит флаг и останавливает ход ──
#[tokio::test]
async fn session_cancel_sets_flag_and_stops_turn() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(LoopingSleepyProvider),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let client = serve(cfg, c, s);
    let sid = init_and_session(client.as_ref()).await;

    client
        .send(RpcMessage::request(
            3,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"go"}]}),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    // cancel как notification.
    client
        .send(RpcMessage::notification(
            "session/cancel",
            json!({"sessionId": sid}),
        ))
        .await
        .unwrap();

    let mut stop = String::new();
    for _ in 0..80 {
        if let RpcMessage::Response { id, result } = recv_to(client.as_ref()).await {
            if id == json!(3) {
                stop = result.unwrap()["stopReason"].as_str().unwrap().to_string();
                break;
            }
        }
    }
    assert_eq!(stop, "cancelled", "cancel → ход завершился cancelled");
}

// ── 15. auto: Auto-тир применяется БЕЗ permission ──
#[tokio::test]
async fn auto_autonomy_applies_auto_tier_without_permission() {
    let (c, s) = channel_pair();
    let (dir, db) = open_db().await;
    let canon = dir.path().canonicalize().unwrap();
    let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "n1".into(),
            name: "note.create".into(),
            arguments: r#"{"path":"Notes/A.md","content":"auto"}"#.into(),
        }])),
        Ok(ToolTurn::Final("готово".into())),
    ]));
    let cfg = cfg_with(provider, canon.clone(), &db, true, "auto");
    let client = serve(cfg, c, s);
    let sid = init_and_session(client.as_ref()).await;
    client
        .send(RpcMessage::request(
            3,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"создай"}]}),
        ))
        .await
        .unwrap();

    let mut perm = false;
    let mut stop = String::new();
    for _ in 0..80 {
        match recv_to(client.as_ref()).await {
            RpcMessage::Request { method, .. } if method == "session/request_permission" => {
                perm = true
            }
            RpcMessage::Response { id, result } if id == json!(3) => {
                stop = result.unwrap()["stopReason"].as_str().unwrap().to_string();
                break;
            }
            _ => {}
        }
    }
    assert!(!perm, "Auto-тир под auto НЕ шлёт request_permission");
    assert_eq!(stop, "end_turn");
    assert!(
        std::fs::read_to_string(canon.join("Notes/A.md")).is_ok(),
        "Auto-тир авто-применён БЕЗ permission"
    );
}

// ── 16. EOF без активного хода → serve_acp возвращается ──
#[tokio::test]
async fn eof_drains_cleanly() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(FakeProvider::new(vec![])),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let server = Arc::new(s);
    let h = tokio::spawn(serve_acp(server, cfg));
    drop(c); // закрываем клиента → EOF
    let r = tokio::time::timeout(Duration::from_secs(5), h).await;
    assert!(r.is_ok(), "serve_acp вернулся по EOF (без зависа)");
}

// ── 17. слишком большой prompt → -32602 ──
#[tokio::test]
async fn oversized_prompt_rejected() {
    let (c, s) = channel_pair();
    let (_d, db) = open_db().await;
    let cfg = cfg_with(
        Arc::new(FakeProvider::new(vec![])),
        _d.path().to_path_buf(),
        &db,
        false,
        "confirm",
    );
    let client = serve(cfg, c, s);
    let sid = init_and_session(client.as_ref()).await;
    let big = "x".repeat(MAX_PROMPT_BYTES + 1);
    let r = request(
        client.as_ref(),
        3,
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type":"text","text": big}]}),
    )
    .await;
    assert_eq!(
        r.unwrap_err().code,
        -32602,
        "prompt > 256KiB → invalid_params"
    );
}

// ── 18. чистые функции ──
#[test]
fn map_event_assistant_token() {
    let v = map_event_to_acp("s1", &AgentEvent::AssistantToken("hi".into()));
    assert_eq!(v.len(), 1);
    match &v[0] {
        RpcMessage::Notification { method, params } => {
            assert_eq!(method, "session/update");
            assert_eq!(params["sessionId"], "s1");
            assert_eq!(params["update"]["sessionUpdate"], "agent_message_chunk");
            assert_eq!(params["update"]["content"]["text"], "hi");
        }
        _ => panic!(),
    }
}

#[test]
fn map_event_tool_call_and_result() {
    let call = map_event_to_acp(
        "s1",
        &AgentEvent::ToolCall {
            id: "t1".into(),
            kind: "note.create".into(),
            args: "{}".into(),
        },
    );
    match &call[0] {
        RpcMessage::Notification { params, .. } => {
            assert_eq!(params["update"]["sessionUpdate"], "tool_call");
            assert_eq!(params["update"]["toolCallId"], "t1");
            assert_eq!(params["update"]["kind"], "edit"); // note.create → write → edit
            assert_eq!(params["update"]["status"], "in_progress");
        }
        _ => panic!(),
    }
    let ok = map_event_to_acp(
        "s1",
        &AgentEvent::ToolResult {
            id: "t1".into(),
            content: "done".into(),
            is_error: false,
        },
    );
    assert_eq!(notif_params(&ok[0])["update"]["status"], "completed");
    let err = map_event_to_acp(
        "s1",
        &AgentEvent::ToolResult {
            id: "t1".into(),
            content: "boom".into(),
            is_error: true,
        },
    );
    assert_eq!(notif_params(&err[0])["update"]["status"], "failed");
}

#[test]
fn map_event_plan_proposed() {
    let v = map_event_to_acp(
        "s1",
        &AgentEvent::PlanProposed {
            run_id: 1,
            steps: vec![
                PlanStep {
                    id: "a".into(),
                    label: "research".into(),
                    status: PlanStepState::Running,
                },
                PlanStep {
                    id: "b".into(),
                    label: "write".into(),
                    status: PlanStepState::Failed,
                },
            ],
        },
    );
    let p = notif_params(&v[0]);
    assert_eq!(p["update"]["sessionUpdate"], "plan");
    assert_eq!(p["update"]["entries"][0]["status"], "in_progress");
    assert_eq!(p["update"]["entries"][1]["status"], "completed"); // Failed → completed
}

#[test]
fn map_event_empties() {
    for ev in [
        AgentEvent::Final("x".into()),
        AgentEvent::Proposal {
            run_id: 1,
            files: vec![],
        },
        AgentEvent::Diff {
            path: "a".into(),
            add: 1,
            del: 0,
            status: crate::agent::event::FileStatus::New,
        },
        AgentEvent::ContextUsage { used: 1, window: 2 },
        AgentEvent::PlanStepStatus {
            id: "x".into(),
            status: PlanStepState::Done,
        },
        AgentEvent::SubagentStatus {
            parent_run_id: 1,
            child_run_id: 2,
            goal: "g".into(),
            status: crate::agent::event::SubagentState::Done,
            summary: None,
        },
    ] {
        assert!(map_event_to_acp("s1", &ev).is_empty(), "{ev:?} → пусто");
    }
    // Error → один chunk с [error] (НЕ пусто).
    assert_eq!(
        map_event_to_acp("s1", &AgentEvent::Error("boom".into())).len(),
        1
    );
}

fn sample_batch() -> ProposalBatch {
    use crate::actuator::classify::{ConfirmReason, RiskTier};
    ProposalBatch {
        run_id: 7,
        items: vec![
            ProposalItem {
                action_id: 10,
                target_rel: "A.md".into(),
                tier: RiskTier::Confirm(ConfirmReason::LargeOverwrite),
                add: 3,
                del: 1,
            },
            ProposalItem {
                action_id: 20,
                target_rel: "B.md".into(),
                tier: RiskTier::Auto,
                add: 2,
                del: 0,
            },
        ],
    }
}

#[test]
fn proposal_to_permission_params_shape() {
    let p = proposal_to_permission_params("s1", 7, PERM_ID_BASE, &sample_batch());
    assert_eq!(p["sessionId"], "s1");
    assert_eq!(
        p["toolCall"]["toolCallId"],
        format!("run7-perm{PERM_ID_BASE}")
    );
    assert_eq!(p["toolCall"]["kind"], "edit");
    let content = p["toolCall"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2, "2-айтемный батч → 2 diff-записи");
    assert_eq!(content[0]["path"], "A.md");
    assert_eq!(content[0]["newText"], ""); // деградированный diff (R4)
    let opts = p["options"].as_array().unwrap();
    assert_eq!(opts.len(), 2);
    assert_eq!(opts[0]["optionId"], "allow");
    assert_eq!(opts[1]["optionId"], "reject");
    // title несёт суммарные +/-.
    assert!(p["toolCall"]["title"].as_str().unwrap().contains("+5/-1"));
}

#[test]
fn outcome_to_batch_decision_cases() {
    let b = sample_batch();
    // selected+allow → одобряет ВСЕ айтемы.
    let allow = outcome_to_batch_decision(
        &b,
        &Ok(json!({"outcome": {"outcome": "selected", "optionId": "allow"}})),
    );
    assert!(allow.is_approved(10) && allow.is_approved(20));
    // reject / unknown / cancelled / Err → reject_all.
    for r in [
        Ok(json!({"outcome": {"outcome": "selected", "optionId": "reject"}})),
        Ok(json!({"outcome": {"outcome": "selected", "optionId": "weird"}})),
        Ok(json!({"outcome": {"outcome": "cancelled"}})),
        Err(RpcError::internal("x")),
    ] {
        let d = outcome_to_batch_decision(&b, &r);
        assert!(
            !d.is_approved(10) && !d.is_approved(20),
            "fail-closed reject_all для {r:?}"
        );
    }
}

#[test]
fn stopreason_mapping() {
    assert_eq!(
        stopreason_from_outcome(&LoopOutcome::Final("x".into())),
        "end_turn"
    );
    assert_eq!(
        stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Cancelled,
            partial: String::new()
        }),
        "cancelled"
    );
    assert_eq!(
        stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Paused,
            partial: String::new()
        }),
        "cancelled"
    );
    assert_eq!(
        stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Tokens,
            partial: String::new()
        }),
        "max_turn_requests"
    );
    assert_eq!(
        stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Steps,
            partial: String::new()
        }),
        "max_turn_requests"
    );
    assert_eq!(
        stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
            kind: BudgetKind::WallClock,
            partial: String::new()
        }),
        "max_turn_requests"
    );
    assert_eq!(
        stopreason_from_outcome(&LoopOutcome::Error("e".into())),
        "refusal"
    );
}

/// R-2 ХАРАКТЕРИЗАЦИЯ (фикстура «до/после» дедупа): полная таблица вариант → (статус, текст)
/// ЭТОГО вызывателя (канон с параметрами ACP-сервера: FinalizeError + «прогон отменён»), точным
/// сравнением (байт-в-байт). Тексты попадают в run_store/историю прогонов/UI — канонизация R-2
/// обязана сохранить их без изменений; ассерты идентичны фикстуре «до» на локальной копии.
#[test]
fn outcome_to_finish_characterization_full_table() {
    use crate::agent::run_store::{STATUS_CANCELLED, STATUS_DONE, STATUS_ERROR};
    let be = |kind: BudgetKind| LoopOutcome::BudgetExhausted {
        kind,
        partial: "часть".into(),
    };
    let table: [(LoopOutcome, &str, &str); 7] = [
        (LoopOutcome::Final("итог".into()), STATUS_DONE, "итог"),
        (
            be(BudgetKind::Cancelled),
            STATUS_CANCELLED,
            "прогон отменён; частичный ответ: часть",
        ),
        (
            be(BudgetKind::Paused),
            STATUS_ERROR,
            "прогон приостановлен (kill-switch); частичный ответ: часть",
        ),
        (
            be(BudgetKind::Steps),
            STATUS_ERROR,
            "бюджет исчерпан (Steps); частичный ответ: часть",
        ),
        (
            be(BudgetKind::WallClock),
            STATUS_ERROR,
            "бюджет исчерпан (WallClock); частичный ответ: часть",
        ),
        (
            be(BudgetKind::Tokens),
            STATUS_ERROR,
            "бюджет исчерпан (Tokens); частичный ответ: часть",
        ),
        (LoopOutcome::Error("упал".into()), STATUS_ERROR, "упал"),
    ];
    for (outcome, want_status, want_text) in table {
        let (status, text) = outcome_to_finish(
            &outcome,
            PausePolicy::FinalizeError,
            CancelWording::RunCancelled,
        )
        .expect_finalize();
        assert_eq!(
            (status, text.as_str()),
            (want_status, want_text),
            "вариант: {outcome:?}"
        );
    }
}

#[test]
fn concat_prompt_text_joins_text_blocks() {
    use super::super::schema::ContentBlock;
    let t = concat_prompt_text(&[
        ContentBlock::Text { text: "a".into() },
        ContentBlock::Other,
        ContentBlock::Text { text: "b".into() },
    ]);
    assert_eq!(t, "a\nb");
}
