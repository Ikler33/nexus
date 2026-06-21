//! `ProxyToolProvider` — tool-capable провайдер для прогона ВНУТРИ песочницы (SANDBOX-4a).
//!
//! Контейнер бежит `--network=none` → провайдер НЕ может открыть `reqwest` к LLM. Вместо `OpenAiToolProvider`
//! (host, streaming через `GuardedClient`) in-sandbox-прогон использует `ProxyToolProvider`: тот же
//! OpenAI-запрос, но **`stream:false`** (буферизованный единый JSON-ответ), отправленный через
//! [`super::proxy::ProxyGuardedClient`] (SANDBOX-2) поверх AF_UNIX → host `GuardedProxy` ре-эмитит его
//! через настоящий `GuardedClient` (chokepoint цел). Host-провайдер (`OpenAiToolProvider`) НЕ трогается —
//! горячий стрим-путь стабилен; in-sandbox-путь буферизует (токены не инкрементальны — приемлемо для
//! каркаса, спека §2/§4.2). Парс — НЕ-стрим OpenAI-ответ (`choices[0].message.{content,tool_calls}`).
//!
//! `endpoint` — НАСТОЯЩИЙ URL LLM (напр. `http://127.0.0.1:8080`): контейнер лишь передаёт строку host'у,
//! который коннектится со своей сети (где этот хост жив). Сам контейнер никуда не ходит (нет NIC).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::agent::connect::Transport;
use crate::agent::tool::{ToolCall, ToolSpec};
use crate::ai::tools::{tool_spec_to_json, ToolCapableProvider, ToolTurn};
use crate::ai::{api_base, AiError, AiResult, ChatMessage};
use crate::net::{EgressFeature, RunCtx};

use super::proxy::ProxyGuardedClient;

/// In-sandbox tool-capable провайдер: запрос через [`ProxyGuardedClient`] (egress.sock), `stream:false`.
pub struct ProxyToolProvider<T: Transport> {
    proxy: ProxyGuardedClient<T>,
    /// Полный endpoint `{base}/v1/chat/completions` (host резолвит этот URL).
    endpoint: String,
    model: String,
    temperature: f32,
}

impl<T: Transport> ProxyToolProvider<T> {
    pub fn new(
        proxy: ProxyGuardedClient<T>,
        base_url: &str,
        model: &str,
        temperature: Option<f32>,
    ) -> Self {
        Self {
            proxy,
            endpoint: format!("{}/v1/chat/completions", api_base(base_url)),
            model: model.to_string(),
            temperature: temperature.unwrap_or(0.3),
        }
    }

    /// Тело запроса: как `OpenAiToolProvider::request_body`, но `stream:false` (буферизованный ответ).
    fn request_body(&self, messages: &[ChatMessage], tools: &[ToolSpec]) -> Value {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
            "temperature": self.temperature,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.iter().map(tool_spec_to_json).collect());
            body["tool_choice"] = serde_json::json!("auto");
        }
        body
    }
}

/// Парс НЕ-стрим OpenAI chat-completion → [`ToolTurn`]. Зеркалит контракт стрим-провайдера
/// (`tools.rs::ToolCallsAcc::finalize`): валидирует имя/JSON-аргументы tool_call, синтезирует id, ловит
/// HTTP-200 error-object. `tool_calls` непусты → ToolCalls; иначе content → Final. Вынесено для тестов.
fn parse_completion(v: &Value) -> AiResult<ToolTurn> {
    // llama.cpp/vLLM возвращают `{error:{message,...}}` СО СТАТУСОМ 200 (context overflow / MTP-баг и
    // т.п.) — иначе это молча стало бы Final("") и замаскировало сбой сервера.
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("ошибка сервера модели");
        return Err(AiError::BadResponse(format!("сервер модели: {msg}")));
    }
    let msg = match v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
    {
        Some(c) => &c["message"],
        None => return Err(AiError::BadResponse("ответ модели без choices".into())),
    };
    if let Some(tcs) = msg["tool_calls"].as_array() {
        if !tcs.is_empty() {
            let mut calls = Vec::with_capacity(tcs.len());
            for (i, tc) in tcs.iter().enumerate() {
                let name = tc["function"]["name"].as_str().unwrap_or("");
                if name.is_empty() {
                    // Как finalize(): без имени → re-askable ошибка (не UnknownTool("")).
                    return Err(AiError::BadResponse("tool_call без имени функции".into()));
                }
                // id: синтез из индекса, если пуст (иначе корреляция ToolCall↔ToolResult схлопнется).
                let id = match tc["id"].as_str() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => format!("call_{i}"),
                };
                // arguments: строка-JSON (валидируем) ИЛИ объект (нестрогие серверы → сериализуем).
                let args = match &tc["function"]["arguments"] {
                    Value::String(s) => {
                        let s = s.trim();
                        if s.is_empty() {
                            "{}".to_string()
                        } else {
                            serde_json::from_str::<Value>(s).map_err(|e| {
                                AiError::BadResponse(format!(
                                    "tool_call '{name}' аргументы не JSON: {e}"
                                ))
                            })?;
                            s.to_string()
                        }
                    }
                    obj @ Value::Object(_) => obj.to_string(),
                    Value::Null => "{}".to_string(),
                    other => {
                        return Err(AiError::BadResponse(format!(
                            "tool_call '{name}' аргументы неожиданного типа: {other}"
                        )))
                    }
                };
                calls.push(ToolCall {
                    id,
                    name: name.to_string(),
                    arguments: args,
                });
            }
            return Ok(ToolTurn::ToolCalls(calls));
        }
    }
    Ok(ToolTurn::Final(
        msg["content"].as_str().unwrap_or("").to_string(),
    ))
}

#[async_trait]
impl<T: Transport> ToolCapableProvider for ProxyToolProvider<T> {
    async fn stream_chat_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        on_token: &mut (dyn FnMut(String) + Send),
        cancel: &Arc<AtomicBool>,
        _ctx: RunCtx, // run_id штампует host-side GuardedProxy (не из контейнера) — _ctx не нужен здесь
    ) -> AiResult<ToolTurn> {
        if cancel.load(Ordering::Relaxed) {
            return Err(AiError::Http("запрос отменён".into()));
        }
        let body = self.request_body(messages, tools);
        let resp = self
            .proxy
            .post_json(&self.endpoint, EgressFeature::Chat, &body)
            .await
            .map_err(|e| AiError::Http(format!("sandbox egress: {}", e.message)))?;
        if !(200..300).contains(&resp.status) {
            return Err(AiError::Http(format!("статус {}", resp.status)));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(AiError::Http("запрос отменён".into()));
        }
        let v: Value = serde_json::from_str(&resp.body)
            .map_err(|e| AiError::BadResponse(format!("парс ответа модели: {e}")))?;
        let turn = parse_completion(&v)?;
        // Буферизованный путь: финальный контент эмитим разом (не инкрементально — каркас).
        if let ToolTurn::Final(ref content) = turn {
            if !content.is_empty() {
                on_token(content.clone());
            }
        }
        Ok(turn)
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::connect::channel_pair;
    use crate::sandbox::proxy::{BackendResponse, EgressBackend, EgressBudget, GuardedProxy, Verb};
    use std::sync::Mutex;

    /// Мок egress-бэкенда: возвращает заданное JSON-тело как ответ модели (status 200).
    struct CannedBackend {
        body: String,
        last_body_out: Mutex<Option<Value>>,
    }
    #[async_trait]
    impl EgressBackend for Arc<CannedBackend> {
        async fn fetch(
            &self,
            _verb: Verb,
            _url: &str,
            _feature: EgressFeature,
            body: Option<&Value>,
            _ctx: RunCtx,
        ) -> Result<BackendResponse, crate::net::NetError> {
            *self.last_body_out.lock().unwrap() = body.cloned();
            Ok(BackendResponse {
                status: 200,
                content_type: Some("application/json".into()),
                body: self.body.clone().into_bytes(),
            })
        }
    }

    /// Поднимает GuardedProxy(mock) на host-конце канала, возвращает client-transport для шима.
    fn spawn_proxy(body: &str) -> (crate::agent::connect::ChannelTransport, Arc<CannedBackend>) {
        let (client_t, host_t) = channel_pair();
        let backend = Arc::new(CannedBackend {
            body: body.to_string(),
            last_body_out: Mutex::new(None),
        });
        let proxy = GuardedProxy::new(
            backend.clone(),
            7,
            vec![EgressFeature::Chat],
            EgressBudget::new(u64::MAX, u32::MAX),
        );
        tokio::spawn(async move {
            while let Some(msg) = host_t.recv().await {
                if let crate::agent::connect::RpcMessage::Request { id, method, params } = msg {
                    let result = proxy.handle(&method, params).await;
                    let _ = host_t
                        .send(crate::agent::connect::RpcMessage::Response { id, result })
                        .await;
                }
            }
        });
        (client_t, backend)
    }

    fn provider(
        client_t: crate::agent::connect::ChannelTransport,
    ) -> ProxyToolProvider<crate::agent::connect::ChannelTransport> {
        ProxyToolProvider::new(
            ProxyGuardedClient::new(client_t),
            "http://llm:8080",
            "qwen",
            None,
        )
    }

    #[test]
    fn request_body_stream_false_with_tools() {
        let (client_t, _h) = channel_pair();
        let p = provider(client_t);
        let body = p.request_body(&[], &[]);
        assert_eq!(
            body["stream"], false,
            "in-sandbox путь буферизует (stream:false)"
        );
        assert!(body.get("tools").is_none(), "пустые tools → без поля");
    }

    #[test]
    fn parse_completion_tool_calls_and_final() {
        let tc = serde_json::json!({"choices":[{"message":{"tool_calls":[
            {"id":"a1","type":"function","function":{"name":"note.create","arguments":"{\"path\":\"X.md\"}"}}
        ]}}]});
        match parse_completion(&tc).unwrap() {
            ToolTurn::ToolCalls(c) => {
                assert_eq!(c[0].name, "note.create");
                assert_eq!(c[0].id, "a1");
                assert!(c[0].arguments.contains("X.md"));
            }
            _ => panic!("ожидали ToolCalls"),
        }
        let fin = serde_json::json!({"choices":[{"message":{"content":"привет"}}]});
        assert_eq!(
            parse_completion(&fin).unwrap(),
            ToolTurn::Final("привет".into())
        );
    }

    #[test]
    fn parse_completion_multiple_tool_calls_ordered() {
        let v = serde_json::json!({"choices":[{"message":{"tool_calls":[
            {"id":"a","function":{"name":"first","arguments":"{}"}},
            {"id":"b","function":{"name":"second","arguments":"{}"}}
        ]}}]});
        match parse_completion(&v).unwrap() {
            ToolTurn::ToolCalls(c) => {
                assert_eq!(c.len(), 2);
                assert_eq!(c[0].name, "first");
                assert_eq!(c[1].name, "second");
            }
            _ => panic!("ожидали 2 ToolCalls"),
        }
    }

    #[test]
    fn parse_completion_http200_error_object_is_bad_response() {
        // llama.cpp/vLLM: {error:{...}} со статусом 200 — НЕ должно стать Final("").
        let v = serde_json::json!({"error":{"message":"context overflow","type":"server"}});
        let r = parse_completion(&v);
        assert!(matches!(r, Err(AiError::BadResponse(ref m)) if m.contains("context overflow")));
    }

    #[test]
    fn parse_completion_no_choices_is_bad_response() {
        assert!(matches!(
            parse_completion(&serde_json::json!({})),
            Err(AiError::BadResponse(_))
        ));
    }

    #[test]
    fn parse_completion_invalid_args_is_bad_response() {
        let v = serde_json::json!({"choices":[{"message":{"tool_calls":[
            {"id":"a","function":{"name":"t","arguments":"{not json"}}
        ]}}]});
        assert!(matches!(parse_completion(&v), Err(AiError::BadResponse(_))));
    }

    #[test]
    fn parse_completion_missing_name_is_bad_response() {
        let v = serde_json::json!({"choices":[{"message":{"tool_calls":[
            {"id":"a","function":{"arguments":"{}"}}
        ]}}]});
        assert!(matches!(parse_completion(&v), Err(AiError::BadResponse(_))));
    }

    #[test]
    fn parse_completion_synthesizes_missing_id_and_object_args() {
        // Нестрогий сервер: нет id + arguments как ОБЪЕКТ (не строка).
        let v = serde_json::json!({"choices":[{"message":{"tool_calls":[
            {"function":{"name":"echo","arguments":{"text":"hi"}}}
        ]}}]});
        match parse_completion(&v).unwrap() {
            ToolTurn::ToolCalls(c) => {
                assert_eq!(c[0].id, "call_0", "id синтезирован из индекса");
                assert!(
                    c[0].arguments.contains("\"text\""),
                    "объект-args сериализован"
                );
            }
            _ => panic!("ожидали ToolCalls"),
        }
    }

    #[tokio::test]
    async fn stream_chat_tools_final_over_proxy() {
        let (client_t, _b) = spawn_proxy(r#"{"choices":[{"message":{"content":"итог"}}]}"#);
        let p = provider(client_t);
        let mut tokens = String::new();
        let mut on = |s: String| tokens.push_str(&s);
        let cancel = Arc::new(AtomicBool::new(false));
        let turn = p
            .stream_chat_tools(&[], &[], &mut on, &cancel, RunCtx::NONE)
            .await
            .unwrap();
        assert_eq!(turn, ToolTurn::Final("итог".into()));
        assert_eq!(tokens, "итог", "контент эмитнут в on_token");
    }

    #[tokio::test]
    async fn stream_chat_tools_toolcalls_over_proxy() {
        let body = r#"{"choices":[{"message":{"tool_calls":[{"id":"c1","type":"function","function":{"name":"echo","arguments":"{\"text\":\"hi\"}"}}]}}]}"#;
        let (client_t, backend) = spawn_proxy(body);
        let p = provider(client_t);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut on = |_: String| {};
        let turn = p
            .stream_chat_tools(&[], &[], &mut on, &cancel, RunCtx::NONE)
            .await
            .unwrap();
        assert!(matches!(turn, ToolTurn::ToolCalls(ref c) if c[0].name == "echo"));
        // запрос ушёл через прокси (stream:false долетел до бэкенда).
        let sent = backend.last_body_out.lock().unwrap().clone().unwrap();
        assert_eq!(sent["stream"], false);
    }

    #[tokio::test]
    async fn empty_body_is_bad_response() {
        let (client_t, _b) = spawn_proxy("");
        let p = provider(client_t);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut on = |_: String| {};
        assert!(matches!(
            p.stream_chat_tools(&[], &[], &mut on, &cancel, RunCtx::NONE)
                .await,
            Err(AiError::BadResponse(_))
        ));
    }

    #[tokio::test]
    async fn cancel_before_send_returns_err() {
        let (client_t, _b) = spawn_proxy(r#"{"choices":[{"message":{"content":"x"}}]}"#);
        let p = provider(client_t);
        let cancel = Arc::new(AtomicBool::new(true));
        let mut on = |_: String| {};
        assert!(p
            .stream_chat_tools(&[], &[], &mut on, &cancel, RunCtx::NONE)
            .await
            .is_err());
    }
}
