//! Tool-capable chat-провайдер (AGENT-1) — ОТДЕЛЬНЫЙ от [`super::chat::OpenAiChatProvider`] тип.
//!
//! # Почему отдельный тип (I-5 / ADR-005, ADR-009)
//! Tools НЕ добавляются полем в `OpenAiChatProvider` (это был бы отвергнутый вариант I-5/ADR-005):
//! так tool-calling НИКОГДА не протекает в plain-chat/web/news/websearch путь. [`ToolCapableProvider`]
//! — РАЗДЕЛЬНЫЙ трейт (не `ChatProvider`); chat-путь и его eval-гейты остаются нетронутыми (P0-d).
//! Инвариант стережёт grep-линт `scripts/check-tooluse.mjs`: `OpenAiToolProvider` упоминается ТОЛЬКО в
//! этом файле, `agent/`, `nexus-agentd/` и тестах.
//!
//! # SSE tool_calls
//! Стрим OpenAI-совместимого сервера отдаёт `tool_calls` ФРАГМЕНТАМИ, склеиваемыми по `index`
//! (`delta.tool_calls[].index`): первый фрагмент несёт `id`+`function.name`, последующие дописывают
//! `function.arguments` по байтам. Аккумулятор [`ToolCallsAcc`] копит их в `BTreeMap<usize, _>` и
//! ФИНАЛИЗИРУЕТ ход на `finish_reason == "tool_calls"` (НЕ на `[DONE]`). Контент по-прежнему стримится
//! в `on_token` (может чередоваться с tool_calls). Аккумулятор — чистая функция от строк SSE → офлайн-тест.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use super::chat::ChatMessage;
use super::{AiError, AiResult};
use crate::agent::tool::{ToolCall, ToolSpec};
use crate::net::{EgressFeature, GuardedClient, RunCtx};

/// Idle-таймаут стрима ПОСЛЕ первого байта (зеркалит `chat.rs`): нет чанка за это время → рвём (не
/// виснем вечно). INFER-CFG: дефолт; конфигурируется `ChatConfig::idle_timeout()`.
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Таймаут ПЕРВОГО токена (зеркалит `chat.rs`, INFER-CFG cold-start): инициация стрима + чанки ДО
/// первого полученного байта (переживает 1–3-минутный cold-start V100). ПОСЛЕ первого байта действует
/// [`STREAM_IDLE_TIMEOUT`]. Конфигурируется `ChatConfig::first_token_timeout()`.
const STREAM_FIRST_TOKEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Исход одного хода tool-capable модели: либо она запросила инструменты, либо дала финальный ответ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolTurn {
    /// Модель запросила вызовы инструментов (finish_reason=tool_calls). Цикл исполнит их и продолжит.
    ToolCalls(Vec<ToolCall>),
    /// Модель завершила ход контентным ответом (без новых tool_call) — это финал.
    Final(String),
}

/// Провайдер, СПОСОБНЫЙ к tool-calling (AGENT-1). РАЗДЕЛЬНЫЙ трейт от [`super::chat::ChatProvider`]
/// (chat-путь tool-free). Реализуется [`OpenAiToolProvider`]; мокается в тестах цикла.
#[async_trait]
pub trait ToolCapableProvider: Send + Sync {
    /// Один ход: шлёт `messages` + `tools` модели, СТРИМИТ контент в `on_token`, возвращает [`ToolTurn`]
    /// (запрошенные инструменты ИЛИ финальный текст). `cancel` прерывает (партиал дропается, без исполнения).
    ///
    /// `ctx` — per-call run-контекст (AGENT-3a): эгресс этого хода аудитится с `ctx.run_id`. Провайдер
    /// СТАТЕЛЕССЕН — run-контекст НЕ хранится на нём, а едет по каналу вызова (как `cancel`), поэтому
    /// конкурентные прогоны через ОДИН провайдер атрибутируют эгресс независимо (каждый несёт свой `ctx`).
    async fn stream_chat_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        on_token: &mut (dyn FnMut(String) + Send),
        cancel: &Arc<AtomicBool>,
        ctx: RunCtx,
    ) -> AiResult<ToolTurn>;

    /// Идентификатор модели (для истории/диагностики).
    fn model_id(&self) -> &str;
}

/// Tool-calling через OpenAI-совместимый `POST {base}/v1/chat/completions` со `stream:true`.
/// Те же поля/конструкция, что у `OpenAiChatProvider`, но СВОЁ тело запроса (с `tools`/`tool_choice`)
/// и СВОЙ парсер потока (аккумулятор tool_calls). Эгресс — через [`GuardedClient`] (та же граница).
pub struct OpenAiToolProvider {
    client: GuardedClient,
    feature: EgressFeature,
    endpoint: String,
    model: String,
    temperature: f32,
    /// Таймаут ПЕРВОГО токена (INFER-CFG cold-start): инициация + чанки ДО первого байта.
    first_token_timeout: std::time::Duration,
    /// Idle-таймаут стрима ПОСЛЕ первого байта.
    idle_timeout: std::time::Duration,
}

impl OpenAiToolProvider {
    /// Конструкция как у `OpenAiChatProvider::new` (guarded-клиент + feature-тег + base/model/temp).
    pub fn new(
        client: &GuardedClient,
        feature: EgressFeature,
        base_url: &str,
        model: &str,
        temperature: Option<f32>,
    ) -> Self {
        Self {
            client: client.clone(),
            feature,
            endpoint: format!("{}/v1/chat/completions", crate::ai::api_base(base_url)),
            model: model.to_string(),
            temperature: temperature.unwrap_or(0.3),
            first_token_timeout: STREAM_FIRST_TOKEN_TIMEOUT,
            idle_timeout: STREAM_IDLE_TIMEOUT,
        }
    }

    /// Тело запроса tool-провайдера: ОТДЕЛЬНЫЙ билдер (не мутирует `OpenAiChatProvider::request_body`).
    /// Добавляет `tools` (OpenAI function-schema из [`ToolSpec`]) + `tool_choice:"auto"`. Пустой набор
    /// инструментов → поля не добавляются (обычный chat-запрос).
    fn request_body(&self, messages: &[ChatMessage], tools: &[ToolSpec]) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "temperature": self.temperature,
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools.iter().map(tool_spec_to_json).collect());
            body["tool_choice"] = serde_json::json!("auto");
        }
        body
    }

    /// INFER-CFG: таймаут первого токена (cold-start). Из `ChatConfig::first_token_timeout()`.
    pub fn with_first_token_timeout(mut self, d: std::time::Duration) -> Self {
        self.first_token_timeout = d;
        self
    }

    /// INFER-CFG: idle-таймаут стрима после первого байта. Из `ChatConfig::idle_timeout()`. Также
    /// тест-хелпер (быстрый обрыв залипшего сервера в офлайн-тестах).
    pub fn with_idle_timeout(mut self, d: std::time::Duration) -> Self {
        self.idle_timeout = d;
        self
    }
}

/// [`ToolSpec`] → OpenAI `tools[]`-элемент (`{type:"function", function:{name,description,parameters}}`).
fn tool_spec_to_json(spec: &ToolSpec) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.parameters,
        }
    })
}

#[async_trait]
impl ToolCapableProvider for OpenAiToolProvider {
    async fn stream_chat_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        on_token: &mut (dyn FnMut(String) + Send),
        cancel: &Arc<AtomicBool>,
        ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        let body = self.request_body(messages, tools);
        // Инициация стрима (post + статус) ДО первого байта — без ретрая здесь (AGENT-1 держит цикл
        // простым; P0-d-ретрай живёт в chat-пути). Idle-таймаут страхует от залипшего коннекта.
        // `ctx` коррелирует ЭТОТ эгресс на прогон (per-call, не глобальный слот; AGENT-3a).
        let send_fut = self
            .client
            .post_json(&self.endpoint, self.feature, &body, ctx);
        // INFER-CFG: инициация = ДО первого байта → first_token_timeout (переживает cold-start V100).
        let resp = match tokio::time::timeout(self.first_token_timeout, send_fut).await {
            Err(_) => {
                return Err(AiError::Http(
                    "таймаут ответа модели (сервер не отвечает)".into(),
                ))
            }
            Ok(r) => r.map_err(AiError::from)?,
        };
        let status = resp.status();
        if !status.is_success() {
            return Err(AiError::Http(format!("статус {status}")));
        }

        let mut resp = resp;
        let mut acc = ToolCallsAcc::default();
        let mut content = String::new();
        let mut buf: Vec<u8> = Vec::new();
        // INFER-CFG cold-start стейт-машина (зеркалит chat.rs): ДО первого полученного байта таймаут
        // чанка = first_token_timeout (cold-start V100 задерживает первый `data:`-чанк на 1–3 мин),
        // ПОСЛЕ первого байта → idle_timeout (детект зависшего steady-state стрима).
        let mut got_first_byte = false;
        loop {
            let chunk_timeout = if got_first_byte {
                self.idle_timeout
            } else {
                self.first_token_timeout
            };
            let Some(chunk) = tokio::time::timeout(chunk_timeout, resp.chunk())
                .await
                .map_err(|_| AiError::Http("таймаут стрима модели (нет данных)".into()))?
                .map_err(|e| AiError::Http(e.to_string()))?
            else {
                break;
            };
            // Первый НЕПУСТОЙ чанк переключает таймаут на idle (steady-state).
            if !chunk.is_empty() {
                got_first_byte = true;
            }
            if cancel.load(Ordering::Relaxed) {
                // Отмена посреди стрима: ДРОПАЕМ партиал, НЕ исполняем (контракт edge-кейса).
                return Err(AiError::Http("запрос отменён".into()));
            }
            buf.extend_from_slice(&chunk);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                match parse_tool_sse_line(line.trim_end_matches(['\r', '\n'])) {
                    ToolSseEvent::Content(s) => {
                        content.push_str(&s);
                        on_token(s);
                    }
                    ToolSseEvent::ToolCallFragment(frag) => acc.apply(frag),
                    // Финализация по finish_reason==tool_calls (НЕ по [DONE]).
                    ToolSseEvent::FinishToolCalls => {
                        return acc.finalize().map(ToolTurn::ToolCalls);
                    }
                    ToolSseEvent::FinishStop | ToolSseEvent::Done => {
                        // Корректный финал контента. Если сервер всё же накопил tool_calls без явного
                        // finish_reason=tool_calls (нестрогие реализации) — отдаём их; иначе финал-текст.
                        if !acc.is_empty() {
                            return acc.finalize().map(ToolTurn::ToolCalls);
                        }
                        return Ok(ToolTurn::Final(content));
                    }
                    ToolSseEvent::Other => {}
                }
            }
        }
        // Поток кончился без явного finish/[DONE]: что накопили, то и отдаём.
        if !acc.is_empty() {
            acc.finalize().map(ToolTurn::ToolCalls)
        } else {
            Ok(ToolTurn::Final(content))
        }
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

/// Событие одной SSE-строки tool-стрима.
enum ToolSseEvent {
    /// Дельта контента ответа.
    Content(String),
    /// Фрагмент tool_call (index + опц. id/name + дельта arguments) — копится в аккумуляторе.
    ToolCallFragment(ToolCallFrag),
    /// `finish_reason == "tool_calls"` — финализируем накопленные вызовы.
    FinishToolCalls,
    /// `finish_reason == "stop"` (или иной не-tool финал) — финал контента.
    FinishStop,
    /// `data: [DONE]` — конец потока.
    Done,
    /// Прочее (keep-alive, роль, нераспознанное).
    Other,
}

/// Фрагмент одного tool_call из дельты (index-keyed). `id`/`name` приходят в первом фрагменте индекса,
/// `args` дописываются по кускам (могут резаться по байтам/UTF-8 границам между чанками).
struct ToolCallFrag {
    index: usize,
    id: Option<String>,
    name: Option<String>,
    args: Option<String>,
}

/// Аккумулятор tool_calls по `index` (`BTreeMap` → стабильный порядок по индексу при финализации).
#[derive(Default)]
struct ToolCallsAcc {
    by_index: BTreeMap<usize, AccEntry>,
}

#[derive(Default)]
struct AccEntry {
    id: Option<String>,
    name: Option<String>,
    args: String,
}

impl ToolCallsAcc {
    /// Применяет фрагмент: создаёт/дополняет запись индекса. id/name берём ПЕРВЫЕ непустые (последующие
    /// фрагменты их обычно не повторяют); args КОНКАТЕНИРУЕМ по порядку прихода.
    fn apply(&mut self, frag: ToolCallFrag) {
        let entry = self.by_index.entry(frag.index).or_default();
        if entry.id.is_none() {
            if let Some(id) = frag.id.filter(|s| !s.is_empty()) {
                entry.id = Some(id);
            }
        }
        if entry.name.is_none() {
            if let Some(name) = frag.name.filter(|s| !s.is_empty()) {
                entry.name = Some(name);
            }
        }
        if let Some(args) = frag.args {
            entry.args.push_str(&args);
        }
    }

    /// Накоплены ли какие-либо tool_calls.
    fn is_empty(&self) -> bool {
        self.by_index.is_empty()
    }

    /// Финализирует накопленное в `Vec<ToolCall>`: проверяет, что СКОНКАТЕНИРОВАННЫЕ args — валидный
    /// JSON (иначе ход — ошибка, чтобы НЕ исполнить мис-парснутый вызов). Пустые args → `{}`. id
    /// отсутствует → синтетический (`call_{index}`) для корреляции событий. Имя отсутствует → ошибка.
    fn finalize(self) -> AiResult<Vec<ToolCall>> {
        let mut calls = Vec::with_capacity(self.by_index.len());
        for (index, entry) in self.by_index {
            let name = entry.name.filter(|s| !s.is_empty()).ok_or_else(|| {
                AiError::BadResponse(format!("tool_call[{index}] без имени функции"))
            })?;
            // Пустые args ("" или только пробелы) трактуем как пустой объект (контракт edge-кейса).
            let args_raw = entry.args.trim();
            let arguments = if args_raw.is_empty() {
                "{}".to_string()
            } else {
                // Валидация: args ДОЛЖНЫ быть синтаксически корректным JSON. Кривые (битый/частичный
                // склей) → ошибка хода (вызывающий делает ровно один capped re-ask, см. runner).
                serde_json::from_str::<serde_json::Value>(args_raw).map_err(|e| {
                    AiError::BadResponse(format!(
                        "tool_call[{index}] '{name}': аргументы не JSON: {e}"
                    ))
                })?;
                args_raw.to_string()
            };
            let id = entry
                .id
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("call_{index}"));
            calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
        if calls.is_empty() {
            return Err(AiError::BadResponse(
                "finish_reason=tool_calls, но ни одного валидного вызова не накоплено".into(),
            ));
        }
        Ok(calls)
    }
}

/// Парсит ОДНУ строку SSE tool-стрима. Не-`data:` строки и нераспознанный JSON → `Other`.
fn parse_tool_sse_line(line: &str) -> ToolSseEvent {
    let Some(data) = line.strip_prefix("data:") else {
        return ToolSseEvent::Other;
    };
    let data = data.trim();
    if data == "[DONE]" {
        return ToolSseEvent::Done;
    }

    #[derive(Deserialize)]
    struct StreamChunk {
        choices: Vec<Choice>,
    }
    #[derive(Deserialize)]
    struct Choice {
        #[serde(default)]
        delta: Delta,
        #[serde(default)]
        finish_reason: Option<String>,
    }
    #[derive(Deserialize, Default)]
    struct Delta {
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        tool_calls: Option<Vec<ToolCallDelta>>,
    }
    #[derive(Deserialize)]
    struct ToolCallDelta {
        #[serde(default)]
        index: usize,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        function: Option<FunctionDelta>,
    }
    #[derive(Deserialize)]
    struct FunctionDelta {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        arguments: Option<String>,
    }

    let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) else {
        return ToolSseEvent::Other;
    };
    let Some(choice) = chunk.choices.into_iter().next() else {
        return ToolSseEvent::Other;
    };

    // tool_calls-фрагмент приоритетнее, но в ОДНОЙ строке обычно что-то одно. Контент и фрагмент
    // приходят в РАЗНЫХ чанках, поэтому порядок проверок здесь не теряет данные.
    if let Some(tcs) = choice.delta.tool_calls {
        if let Some(tc) = tcs.into_iter().next() {
            let (name, args) = match tc.function {
                Some(f) => (f.name, f.arguments),
                None => (None, None),
            };
            return ToolSseEvent::ToolCallFragment(ToolCallFrag {
                index: tc.index,
                id: tc.id,
                name,
                args,
            });
        }
    }
    if let Some(s) = choice.delta.content.filter(|s| !s.is_empty()) {
        return ToolSseEvent::Content(s);
    }
    // finish_reason проверяем ПОСЛЕ дельт (в финальном чанке дельта пуста, есть только finish_reason).
    match choice.finish_reason.as_deref() {
        Some("tool_calls") => ToolSseEvent::FinishToolCalls,
        Some("stop") | Some("length") => ToolSseEvent::FinishStop,
        _ => ToolSseEvent::Other,
    }
}

/// AGENT-1 (I-5): ЕДИНЫЙ строитель tool-capable провайдера цикла агента из `ai.chat`-секции конфига.
///
/// Тот же `ai.chat`-хост/модель и тот же `GuardedClient::for_chat` + [`EgressFeature::Chat`], что и
/// chat-провайдер, но ОТДЕЛЬНЫЙ тип [`OpenAiToolProvider`] (tools НЕ протекают в chat-путь, I-5).
/// `None` — нет `ai.chat` / guarded-клиент не построился (агент без живой модели — деградирует чисто).
///
/// ЖИВЁТ ЗДЕСЬ (дом типа, whitelisted `check-tooluse`), чтобы tool-провайдер конструировался ВНУТРИ
/// границы I-5: вызыватели (desktop `commands/agent.rs` — где `OpenAiToolProvider` запрещён линтом)
/// получают уже-собранный `Arc<dyn ToolCapableProvider>`, не упоминая концретный тип. Зеркало
/// `nexus-agentd::build_agent_tools_min` (там копия — agentd намеренно self-contained); desktop же
/// зовёт ЭТОТ общий строитель (reuse, не дубль).
pub fn build_agent_tool_provider(
    cfg: &super::LocalConfig,
    policy: &Arc<crate::net::EgressPolicy>,
    audit: &Arc<crate::net::EgressAudit>,
) -> Option<Arc<dyn ToolCapableProvider>> {
    let chat = cfg.ai.chat.as_ref()?;
    let model = chat.model.clone().unwrap_or_else(|| "chat".to_string());
    let guarded = GuardedClient::for_chat(policy.clone(), audit.clone(), chat.connect_timeout())
        .map_err(|e| tracing::warn!(error = %e, "tool-провайдер агента не инициализирован"))
        .ok()?;
    // INFER-CFG: температура + cold-start-таймауты стрима из конфига (у tool-провайдера нет retry —
    // повторами ходов заведует сам цикл агента). Connect-таймаут — у guarded-клиента выше.
    let provider = OpenAiToolProvider::new(
        &guarded,
        EgressFeature::Chat,
        &chat.url,
        &model,
        Some(chat.temperature()),
    )
    .with_first_token_timeout(chat.first_token_timeout())
    .with_idle_timeout(chat.idle_timeout());
    tracing::info!(model = %model, "tool-capable провайдер агента включён (AGENT-1)");
    Some(Arc::new(provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Прогоняет последовательность SSE-строк через парсер+аккумулятор, как делает стрим-цикл.
    /// Возвращает (накопленный контент, исход аккумулятора при финализации tool_calls / None).
    fn drive(lines: &[&str]) -> (String, Option<AiResult<Vec<ToolCall>>>) {
        let mut acc = ToolCallsAcc::default();
        let mut content = String::new();
        for line in lines {
            match parse_tool_sse_line(line) {
                ToolSseEvent::Content(s) => content.push_str(&s),
                ToolSseEvent::ToolCallFragment(f) => acc.apply(f),
                ToolSseEvent::FinishToolCalls => {
                    return (content, Some(acc.finalize()));
                }
                ToolSseEvent::FinishStop | ToolSseEvent::Done => {
                    if !acc.is_empty() {
                        return (content, Some(acc.finalize()));
                    }
                    return (content, None);
                }
                ToolSseEvent::Other => {}
            }
        }
        if acc.is_empty() {
            (content, None)
        } else {
            (content, Some(acc.finalize()))
        }
    }

    /// Одиночный tool_call: id+name в первом фрагменте, args одним куском, финал по tool_calls.
    #[test]
    fn sse_single_tool_call() {
        let lines = [
            r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"debug.echo","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"text\":\"hi\"}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "data: [DONE]",
        ];
        let (content, out) = drive(&lines);
        assert!(content.is_empty());
        let calls = out.expect("финализировано").expect("валидно");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].name, "debug.echo");
        assert_eq!(calls[0].arguments, r#"{"text":"hi"}"#);
    }

    /// Контент чередуется с tool_calls (интерливинг) — обе ветви накапливаются независимо.
    #[test]
    fn sse_interleaved_content_and_tool_call() {
        let lines = [
            r#"data: {"choices":[{"delta":{"content":"Сейчас "}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"debug.echo","arguments":"{"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"проверю"}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"text\":\"x\"}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        let (content, out) = drive(&lines);
        assert_eq!(content, "Сейчас проверю");
        let calls = out.unwrap().unwrap();
        assert_eq!(calls[0].arguments, r#"{"text":"x"}"#);
    }

    /// args режутся по байтам через несколько фрагментов — конкатенация даёт валидный JSON.
    #[test]
    fn sse_split_args_across_fragments() {
        let lines = [
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"t","arguments":"{\"a\":"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1,\"b\""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":2}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        let calls = drive(&lines).1.unwrap().unwrap();
        assert_eq!(calls[0].arguments, r#"{"a":1,"b":2}"#);
    }

    /// Несколько tool_calls по разным индексам — финализируются в порядке индекса (BTreeMap).
    #[test]
    fn sse_multi_index_tool_calls() {
        let lines = [
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c1","function":{"name":"second","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c0","function":{"name":"first","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        let calls = drive(&lines).1.unwrap().unwrap();
        assert_eq!(calls.len(), 2);
        // Порядок по index: 0 раньше 1, несмотря на порядок прихода.
        assert_eq!(calls[0].name, "first");
        assert_eq!(calls[1].name, "second");
    }

    /// CRLF-окончания строк (\r\n) обрабатываются как и \n (тримминг \r).
    #[test]
    fn sse_handles_crlf() {
        // drive() уже получает строки без \n; имитируем сохранившийся \r на хвосте data-строки.
        let line = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c\",\"function\":{\"name\":\"t\",\"arguments\":\"{}\"}}]}}]}\r";
        // парсер строк вызывается с trim_end_matches(['\r','\n']) в реальном цикле; эмулируем.
        let trimmed = line.trim_end_matches(['\r', '\n']);
        assert!(matches!(
            parse_tool_sse_line(trimmed),
            ToolSseEvent::ToolCallFragment(_)
        ));
    }

    /// finish_reason приходит ПОЗЖЕ пустой дельтой — финализация именно по нему, не по [DONE].
    #[test]
    fn sse_finalizes_on_finish_reason_not_done() {
        let lines = [
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"t","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            // [DONE] после финализации уже не нужен — финал случился на finish_reason.
            "data: [DONE]",
        ];
        let calls = drive(&lines).1.unwrap().unwrap();
        assert_eq!(calls[0].name, "t");
    }

    /// Пустые args ("") → нормализуются в "{}" (инструмент без аргументов).
    #[test]
    fn sse_empty_args_become_object() {
        let lines = [
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"debug.noop","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        let calls = drive(&lines).1.unwrap().unwrap();
        assert_eq!(calls[0].arguments, "{}");
    }

    /// Битый склей args (невалидный JSON) → финализация ОШИБКА (не мис-исполнение). Вызывающий
    /// делает ровно один re-ask (см. runner).
    #[test]
    fn sse_invalid_json_args_errors() {
        let lines = [
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"t","arguments":"{\"a\":"}}]}}]}"#,
            // финализируем при незакрытом JSON
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        let out = drive(&lines).1.unwrap();
        assert!(out.is_err(), "битый JSON args → ошибка хода, не исполнение");
    }

    /// Re-ask edge (D): args приходят фрагментами с unicode/escape-последовательностями, которые
    /// КОНКАТЕНИРУЮТ в НЕВАЛИДНЫЙ JSON (незакрытая строка-эскейп + оборванный объект). finalize()
    /// ОБЯЗАН вернуть BadResponse — мис-парснутый вызов НЕ исполняется (вызывающий делает один re-ask).
    #[test]
    fn sse_fragmented_unicode_escape_args_invalid_json_errors() {
        let lines = [
            // Открыли объект и строковое значение с валидным \u-эскейпом (кириллица «П»)…
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"t","arguments":"{\"text\":\"\\u041f"}}]}}]}"#,
            // …дописали ещё эскейп + эмодзи, но строку и объект НЕ ЗАКРЫЛИ → склей = битый JSON.
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\\u0440\\ud83d\\ude00 привет"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        let out = drive(&lines).1.expect("финализировано");
        assert!(
            matches!(out, Err(AiError::BadResponse(_))),
            "фрагментированный unicode/escape-склей в невалидный JSON → BadResponse (не ToolCall): {out:?}"
        );
    }

    /// Контр-проба к предыдущему: те же unicode/escape-фрагменты, но КОНКАТЕНИРОВАННЫЕ В ВАЛИДНЫЙ JSON
    /// (строка и объект закрыты) → ровно один ToolCall с сохранёнными сырыми аргументами (граница не
    /// коэрсит, escape-последовательности остаются как есть в сырой строке аргументов).
    #[test]
    fn sse_fragmented_unicode_escape_args_valid_json_ok() {
        let lines = [
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"t","arguments":"{\"text\":\"\\u041f"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\\u0440\\ud83d\\ude00\"}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        let calls = drive(&lines).1.expect("финализировано").expect("валидно");
        assert_eq!(calls.len(), 1);
        // Сырые аргументы хранятся ДОСЛОВНО: \u-escape-последовательности НЕ раскодированы границей
        // (finalize только валидирует и сохраняет raw-строку как пришла от модели).
        assert_eq!(calls[0].arguments, r#"{"text":"\u041f\u0440\ud83d\ude00"}"#);
        // Sanity: это валидный JSON, и при декодировании даёт ожидаемые символы «Пр😀»
        // (П=П, р=р, 😀=😀 — суррогатная пара).
        let v: serde_json::Value =
            serde_json::from_str(&calls[0].arguments).expect("валидный JSON");
        assert_eq!(v["text"], "Пр😀");
    }

    /// Финал по stop без tool_calls → ToolTurn::Final (контент), не вызовы.
    #[test]
    fn sse_finish_stop_is_final_content() {
        let lines = [
            r#"data: {"choices":[{"delta":{"content":"готово"}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ];
        let (content, out) = drive(&lines);
        assert_eq!(content, "готово");
        assert!(out.is_none(), "stop без tool_calls — финал контента");
    }

    /// Тело запроса: пустой набор инструментов → без полей tools/tool_choice; непустой → оба есть.
    #[test]
    fn request_body_adds_tools_only_when_present() {
        let guarded = GuardedClient::unchecked();
        let p = OpenAiToolProvider::new(&guarded, EgressFeature::Chat, "http://x", "qwen", None);
        let no_tools = p.request_body(&[], &[]);
        assert!(no_tools.get("tools").is_none());
        assert!(no_tools.get("tool_choice").is_none());

        let spec = ToolSpec {
            name: "debug.echo".into(),
            description: "echo".into(),
            parameters: serde_json::json!({"type":"object"}),
        };
        let with_tools = p.request_body(&[], std::slice::from_ref(&spec));
        assert_eq!(with_tools["tool_choice"], serde_json::json!("auto"));
        let arr = with_tools["tools"].as_array().expect("tools — массив");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "function");
        assert_eq!(arr[0]["function"]["name"], "debug.echo");
    }

    /// UI-1a: общий строитель `build_agent_tool_provider` — `ai.chat` задан → Some(провайдер с тем же
    /// model_id); секции нет → None (агент без живой модели, деградирует чисто). Сети не касается
    /// (клиент не шлёт запрос при конструкции).
    #[test]
    fn build_agent_tool_provider_some_when_chat_configured() {
        use std::sync::atomic::AtomicBool;
        let policy = std::sync::Arc::new(crate::net::EgressPolicy::new(std::sync::Arc::new(
            AtomicBool::new(false),
        )));
        let audit = std::sync::Arc::new(crate::net::EgressAudit::default());

        let cfg = crate::ai::LocalConfig::parse(
            r#"{"ai":{"chat":{"url":"http://127.0.0.1:9","model":"qwen-tool"}}}"#,
        )
        .unwrap();
        let provider =
            super::build_agent_tool_provider(&cfg, &policy, &audit).expect("ai.chat → провайдер");
        assert_eq!(provider.model_id(), "qwen-tool");

        // Нет ai.chat → None.
        let empty = crate::ai::LocalConfig::parse("{}").unwrap();
        assert!(super::build_agent_tool_provider(&empty, &policy, &audit).is_none());
    }

    /// Отмена посреди стрима → дроп партиала, БЕЗ исполнения (через залипший сервер + взведённый cancel).
    #[tokio::test]
    async fn cancel_mid_stream_drops_partial() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                // Шлём заголовки + один tool_call-фрагмент, затем «зависаем» (cancel должен оборвать).
                let body = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c\",\"function\":{\"name\":\"t\",\"arguments\":\"{}\"}}]}}]}\n\n";
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 9999\r\n\r\n{body}"
                    )
                    .as_bytes(),
                );
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
        let provider = OpenAiToolProvider::new(
            &GuardedClient::unchecked(),
            EgressFeature::Chat,
            &format!("http://{addr}"),
            "qwen",
            Some(0.0),
        )
        .with_idle_timeout(std::time::Duration::from_secs(2));
        let cancel = Arc::new(AtomicBool::new(true)); // взведён сразу → первый же чанк отменяет
        let res = provider
            .stream_chat_tools(
                &[ChatMessage::user("hi")],
                &[],
                &mut |_| {},
                &cancel,
                RunCtx::NONE,
            )
            .await;
        assert!(
            matches!(res, Err(AiError::Http(ref m)) if m.contains("отменён")),
            "отмена → ошибка отмены, без исполнения: {res:?}"
        );
        let _ = server.join();
    }
}
