//! Chat-провайдер (**ADR-005**): отдельная от эмбеддера сущность (другой хост/модель). Стриминг
//! токенов из OpenAI-совместимого `POST /v1/chat/completions` (`stream: true`, SSE).
//!
//! Поток читаем `Response::chunk()` (без фичи `stream`/`futures`): копим байты, режем по `\n`,
//! каждую строку `data: …` парсим в дельту. Прерывание — флагом `cancel` (проверяется по чанкам).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{AiError, AiResult};

/// Сообщение чата (роль + текст). Сериализуется в тело запроса к модели.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// Провайдер чата (ADR-005). Стримит ответ модели токенами.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// Стримит ответ: каждую текстовую дельту отдаёт в `on_token` (по значению — обходит HRTB-
    /// лайфтайм под `async_trait`), возвращает полный текст. При `cancel == true` — прекращает.
    async fn stream_chat(
        &self,
        messages: &[ChatMessage],
        on_token: &mut (dyn FnMut(String) + Send),
        cancel: &Arc<AtomicBool>,
    ) -> AiResult<String>;

    /// Идентификатор модели (для истории/диагностики).
    fn model_id(&self) -> &str;
}

/// Chat через OpenAI-совместимый `POST {base}/v1/chat/completions` (llama.cpp-server, Qwen).
pub struct OpenAiChatProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    temperature: f32,
}

impl OpenAiChatProvider {
    pub fn new(base_url: &str, model: &str, temperature: Option<f32>) -> AiResult<Self> {
        // Без общего timeout: стриминг ответа долгий. Connect-timeout страхует от зависшего коннекта.
        let client = super::core_client_builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| AiError::Http(e.to_string()))?;
        Ok(Self {
            client,
            endpoint: format!("{}/v1/chat/completions", base_url.trim_end_matches('/')),
            model: model.to_string(),
            temperature: temperature.unwrap_or(0.3),
        })
    }
}

#[async_trait]
impl ChatProvider for OpenAiChatProvider {
    async fn stream_chat(
        &self,
        messages: &[ChatMessage],
        on_token: &mut (dyn FnMut(String) + Send),
        cancel: &Arc<AtomicBool>,
    ) -> AiResult<String> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "temperature": self.temperature,
        });
        let mut resp = self
            .client
            .post(&self.endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AiError::Http(format!("статус {}", resp.status())));
        }

        let mut full = String::new();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?
        {
            if cancel.load(Ordering::Relaxed) {
                return Ok(full);
            }
            buf.extend_from_slice(&chunk);
            // Обрабатываем все полные строки (граница `\n` — ASCII, кодпойнты не рвутся).
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                match parse_sse_delta(line.trim_end()) {
                    SseEvent::Content(s) => {
                        full.push_str(&s);
                        on_token(s);
                    }
                    SseEvent::Done => return Ok(full),
                    SseEvent::Other => {}
                }
            }
        }
        Ok(full)
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

/// Событие одной SSE-строки потока чата.
enum SseEvent {
    Content(String),
    Done,
    Other,
}

/// Парсит строку SSE (`data: …`) в дельту контента. Не-`data` строки и нераспознанный JSON → `Other`.
fn parse_sse_delta(line: &str) -> SseEvent {
    let Some(data) = line.strip_prefix("data:") else {
        return SseEvent::Other;
    };
    let data = data.trim();
    if data == "[DONE]" {
        return SseEvent::Done;
    }
    #[derive(Deserialize)]
    struct StreamChunk {
        choices: Vec<Choice>,
    }
    #[derive(Deserialize)]
    struct Choice {
        delta: Delta,
    }
    #[derive(Deserialize)]
    struct Delta {
        content: Option<String>,
    }
    match serde_json::from_str::<StreamChunk>(data) {
        Ok(c) => c
            .choices
            .into_iter()
            .next()
            .and_then(|ch| ch.delta.content)
            .filter(|s| !s.is_empty())
            .map(SseEvent::Content)
            .unwrap_or(SseEvent::Other),
        Err(_) => SseEvent::Other,
    }
}

/// Собирает RAG-сообщения: системная инструкция (отвечать ТОЛЬКО по контексту, цитировать [n],
/// язык вопроса) + пользовательский блок с пронумерованным контекстом и вопросом. `contexts` —
/// пары `(метка-источник, текст-чанка)`.
pub fn build_rag_messages(question: &str, contexts: &[(String, String)]) -> Vec<ChatMessage> {
    const SYSTEM: &str = "Ты — ассистент по личной базе знаний пользователя. Отвечай на вопрос, \
        опираясь ТОЛЬКО на приведённый ниже контекст из заметок. Ссылайся на источники в квадратных \
        скобках вида [1], [2]. Если в контексте нет ответа — честно скажи, что не нашёл его в заметках, \
        и не выдумывай. Отвечай на языке вопроса.";

    let user = if contexts.is_empty() {
        format!("Контекст не найден в заметках.\n\nВопрос: {question}")
    } else {
        let mut ctx = String::new();
        for (i, (source, text)) in contexts.iter().enumerate() {
            ctx.push_str(&format!("[{}] {}\n{}\n\n", i + 1, source, text.trim()));
        }
        format!("Контекст из заметок:\n\n{ctx}Вопрос: {question}")
    };

    vec![ChatMessage::system(SYSTEM), ChatMessage::user(user)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_delta_extracts_content_and_done() {
        let line = r#"data: {"choices":[{"delta":{"content":"Привет"}}]}"#;
        assert!(matches!(parse_sse_delta(line), SseEvent::Content(s) if s == "Привет"));
        assert!(matches!(parse_sse_delta("data: [DONE]"), SseEvent::Done));
        // первый кусок обычно несёт роль без content → Other
        let role = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert!(matches!(parse_sse_delta(role), SseEvent::Other));
        assert!(matches!(parse_sse_delta(": keep-alive"), SseEvent::Other));
        assert!(matches!(parse_sse_delta("data: not-json"), SseEvent::Other));
        assert!(matches!(parse_sse_delta(""), SseEvent::Other));
    }

    #[test]
    fn build_rag_messages_numbers_sources_and_includes_question() {
        let ctx = vec![
            ("Notes/Cat.md".into(), "Кошка спит на коврике.".into()),
            ("Notes/Dog.md".into(), "Собака гуляет.".into()),
        ];
        let msgs = build_rag_messages("Где кошка?", &ctx);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert!(msgs[1].content.contains("[1] Notes/Cat.md"));
        assert!(msgs[1].content.contains("[2] Notes/Dog.md"));
        assert!(msgs[1].content.contains("Где кошка?"));
    }

    #[test]
    fn build_rag_messages_handles_empty_context() {
        let msgs = build_rag_messages("Вопрос?", &[]);
        assert!(msgs[1].content.contains("не найден"));
        assert!(msgs[1].content.contains("Вопрос?"));
    }

    /// Живой стриминг против Qwen на 192.168.0.172:8080 (`cargo test -- --ignored`).
    #[tokio::test]
    #[ignore = "нужен chat-сервер на 192.168.0.172:8080"]
    async fn live_chat_streams_tokens() {
        let provider =
            OpenAiChatProvider::new("http://192.168.0.172:8080", "qwen3", Some(0.0)).unwrap();
        let msgs = vec![ChatMessage::user("Ответь одним словом: столица Франции?")];
        let mut tokens = 0usize;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut on_token = |_: String| tokens += 1;
        let full = provider
            .stream_chat(&msgs, &mut on_token, &cancel)
            .await
            .unwrap();
        assert!(tokens > 0, "должны прийти токены");
        assert!(!full.trim().is_empty(), "накопленный ответ непуст");
        assert!(full.to_lowercase().contains("париж") || full.to_lowercase().contains("paris"));
    }
}
