//! Безопасные СТАБ-инструменты (AGENT-1) — доказывают цикл end-to-end БЕЗ единого побочного эффекта.
//!
//! ZERO actuator: ни fs, ни process, ни net. [`EchoTool`] возвращает свой вход; [`NoopTool`] — read-only
//! «нет операции».
//!
//! **ТОЛЬКО тесты/smoke (B7).** В прод-сессии ([`super::session::run_agent_session`]) стабы НЕ
//! регистрируются: при ВЫКЛ актуаторе реестр записи ПУСТ — модель не видит пустышек в списке
//! инструментов. Потребители — юнит-тесты цикла/реестра, agentd `agent_loop_smoke` и live-эвалы,
//! которые регистрируют их ЯВНО у себя.

use async_trait::async_trait;
use serde::Deserialize;

use super::tool::{Tool, ToolError, ToolSpec};

/// `debug.echo` — возвращает переданный текст. Тривиальная проверка маршрута «модель → инструмент →
/// результат → обратно в промпт». Никаких побочных эффектов.
pub struct EchoTool;

/// Аргументы [`EchoTool`]: ровно одно поле `text`. `deny_unknown_fields` — лишнее поле → BadArgs
/// (I-4 fail-closed, граница не коэрсит мусор).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EchoArgs {
    text: String,
}

#[async_trait]
impl Tool for EchoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "debug.echo".into(),
            description: "Возвращает переданный текст без изменений (диагностический стаб, без \
                          побочных эффектов)."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Текст для возврата" }
                },
                "required": ["text"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        // Пустые аргументы трактуем как «{}» → строгий разбор скажет «отсутствует text» (BadArgs).
        let raw = if args.trim().is_empty() { "{}" } else { args };
        let parsed: EchoArgs =
            serde_json::from_str(raw).map_err(|e| ToolError::BadArgs(e.to_string()))?;
        Ok(parsed.text)
    }
}

/// `debug.noop` — read-only «нет операции»: подтверждает, что инструмент вызван, без чтения чего-либо
/// и без побочных эффектов. Аргументы игнорируются (приёмлет любой JSON / пусто).
pub struct NoopTool;

#[async_trait]
impl Tool for NoopTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "debug.noop".into(),
            description:
                "Ничего не делает и ничего не читает; подтверждает вызов (read-only стаб).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, _args: &str) -> Result<String, ToolError> {
        Ok("ok".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// echo возвращает свой text; лишнее поле → BadArgs (deny_unknown_fields); пусто → BadArgs (нет text).
    #[tokio::test]
    async fn echo_roundtrips_and_failcloses() {
        let t = EchoTool;
        assert_eq!(t.invoke(r#"{"text":"привет"}"#).await.unwrap(), "привет");
        assert!(matches!(
            t.invoke(r#"{"text":"x","extra":1}"#).await,
            Err(ToolError::BadArgs(_))
        ));
        assert!(matches!(t.invoke("").await, Err(ToolError::BadArgs(_))));
        assert!(matches!(
            t.invoke("not json").await,
            Err(ToolError::BadArgs(_))
        ));
    }

    /// noop всегда «ok», аргументы не важны, никаких эффектов.
    #[tokio::test]
    async fn noop_is_readonly_ok() {
        let t = NoopTool;
        assert_eq!(t.invoke("{}").await.unwrap(), "ok");
        assert_eq!(t.invoke("").await.unwrap(), "ok");
        assert_eq!(t.spec().name, "debug.noop");
    }
}
