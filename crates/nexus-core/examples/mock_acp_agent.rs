//! ACP-1 — мок внешнего ACP-агента для CI (независимая реализация контракта, не делит код с клиентом →
//! ловит дрейф схемы как настоящий агент; см. `feedback_mock_must_match_backend`). Говорит line-delimited
//! JSON-RPC 2.0 по stdin/stdout — ровно как `hermes acp`. Прогоняет полный happy-path:
//! `initialize` → `session/new` → `session/prompt`(стрим токена + tool_call + `request_permission`
//! round-trip → `end_turn`). Используется интеграц-тестом `tests/acp_e2e.rs`, который спавнит этот бинарь
//! через [`StdioTransport`] и драйвит его [`AcpClient`]'ом — единственный путь, не покрытый юнитами
//! (реальный подпроцесс + framing + bidirectional read-loop вместе).
//!
//! НАМЕРЕННО синхронный std-IO без рантайма: протокол строго последовательный (запрос→ответ), так проще
//! и нет лишних зависимостей. stdout пайпится → ОБЯЗАТЕЛЕН flush после каждой строки (иначе клиент висит).

use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut lines = stdin.lock().lines();

    while let Some(Ok(line)) = lines.next() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg): Result<Value, _> = serde_json::from_str(line) else {
            continue; // мусор игнорируем (клиент тоже толерантен)
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();

        match method {
            "initialize" => respond(&mut out, id, json!({"protocolVersion": 1})),
            "session/new" => respond(&mut out, id, json!({"sessionId": "s1"})),
            "session/prompt" => drive_prompt(&mut out, &mut lines, id),
            // session/cancel (нотификация) и прочее — завершаемся (ход окончен / клиент уходит).
            "session/cancel" => break,
            _ => {
                if let Some(id) = id {
                    // неизвестный запрос — method_not_found (как настоящий агент)
                    send(
                        &mut out,
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}}),
                    );
                }
            }
        }
    }
}

/// Один ход: стрим токена + tool_call + запрос разрешения (ждём ответ клиента) + финал `end_turn`.
fn drive_prompt(
    out: &mut impl Write,
    lines: &mut std::io::Lines<impl BufRead>,
    prompt_id: Option<Value>,
) {
    // session/update: `update` ВЛОЖЕН (форма реального ACP-агента Hermes 0.17, не flatten).
    // (a) токен ассистента
    notify(
        out,
        json!({"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi "}}}),
    );
    // (a2) ACP-1b: план хода (todo-список) ДО действий — клиент маппит в PlanProposed.
    notify(
        out,
        json!({"sessionId":"s1","update":{"sessionUpdate":"plan","entries":[
            {"content":"edit A and B","priority":"high","status":"in_progress"},
            {"content":"finish","priority":"medium","status":"pending"}
        ]}}),
    );
    // (b) tool_call (edit-намерение)
    notify(
        out,
        json!({"sessionId":"s1","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"edit Notes/A.md",
               "kind":"edit","status":"pending"}}),
    );
    // (c) запрос разрешения с ДВУМЯ diff (мульти-файл, ACP-1b) — БЛОКИРУЕМСЯ до Response клиента.
    send(
        out,
        json!({"jsonrpc":"2.0","id":777,"method":"session/request_permission","params":{
            "sessionId":"s1",
            "toolCall":{"toolCallId":"t1","title":"edit Notes/A.md и Notes/B.md",
                        "content":[{"type":"diff","path":"Notes/A.md","newText":"alpha\nbeta"},
                                   {"type":"diff","path":"Notes/B.md","oldText":"old","newText":"gamma"}]},
            "options":[{"optionId":"a","name":"Allow","kind":"allow_once"},
                       {"optionId":"d","name":"Deny","kind":"reject_once"}]
        }}),
    );
    // читаем Response на 777 (клиент мог сначала прислать что-то иное — отматываем до нужного id)
    let mut approved = false;
    for line in lines.by_ref() {
        let Ok(line) = line else { return };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(resp): Result<Value, _> = serde_json::from_str(line) else {
            continue;
        };
        if resp.get("id").and_then(Value::as_i64) == Some(777) {
            approved = resp
                .pointer("/result/outcome/outcome")
                .and_then(Value::as_str)
                == Some("selected");
            break;
        }
    }
    // (d) ещё токен ПОСЛЕ аппрува (доказывает, что стрим продолжается за permission) + финал
    if approved {
        notify(
            out,
            json!({"sessionId":"s1","update":{"sessionUpdate":"tool_call_update","toolCallId":"t1",
                   "status":"completed","content":[{"type":"content","content":{"type":"text","text":"written"}}]}}),
        );
        notify(
            out,
            json!({"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}),
        );
    }
    let stop = if approved { "end_turn" } else { "cancelled" };
    respond(out, prompt_id, json!({"stopReason": stop}));
}

fn respond(out: &mut impl Write, id: Option<Value>, result: Value) {
    if let Some(id) = id {
        send(out, json!({"jsonrpc":"2.0","id":id,"result":result}));
    }
}

fn notify(out: &mut impl Write, params: Value) {
    send(
        out,
        json!({"jsonrpc":"2.0","method":"session/update","params":params}),
    );
}

/// Пишет один кадр `<json>\n` + flush (stdout пайпится — без flush клиент висит).
fn send(out: &mut impl Write, msg: Value) {
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}
