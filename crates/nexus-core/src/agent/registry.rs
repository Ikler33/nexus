//! [`ToolRegistry`] — реестр инструментов агента, keyed по `spec().name` (AGENT-1).
//!
//! Зеркалит ПАТТЕРН `scheduler::Registry` (HashMap имя→`Arc<dyn …>`), но это РАЗДЕЛЬНЫЙ тип: `Tool` ≠
//! `JobHandler` (разный контракт — `invoke(args)->Result<String,ToolError>` против `handle(&Job)`).

use std::collections::HashMap;
use std::sync::Arc;

use super::event::AgentEvent;
use super::tool::{Tool, ToolCall, ToolError, ToolSpec};

/// Результат диспатча одного вызова инструмента. Цикл превращает его в [`AgentEvent::ToolResult`]
/// (и фенсит `content` при ре-инъекции в промпт).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    /// id вызова (== [`ToolCall::id`]) — корреляция в потоке событий.
    pub id: String,
    /// Текст результата (успех) либо текст ошибки (`is_error`).
    pub content: String,
    /// Инструмент вернул ошибку (неизвестное имя / кривые аргументы / сбой исполнения).
    pub is_error: bool,
}

impl ToolResult {
    /// В [`AgentEvent::ToolResult`] (после фенсинга `content` цикл уже передаёт сюда — см. runner).
    pub fn into_event(self) -> AgentEvent {
        AgentEvent::ToolResult {
            id: self.id,
            content: self.content,
            is_error: self.is_error,
        }
    }
}

/// Реестр инструментов: имя инструмента → реализация. Пустой по умолчанию; инструменты регистрирует
/// композиционный корень (агент-сервис). AGENT-1 кладёт сюда ТОЛЬКО безопасные стабы.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Пустой реестр.
    pub fn new() -> Self {
        Self::default()
    }

    /// Регистрирует инструмент по его `spec().name`. Последняя регистрация под тем же именем
    /// побеждает (как и `HashMap::insert`); возвращает вытесненный инструмент, если был.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) -> Option<Arc<dyn Tool>> {
        let name = tool.spec().name;
        self.tools.insert(name, tool)
    }

    /// Число зарегистрированных инструментов.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Реестр пуст?
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Имена всех зарегистрированных инструментов (отсортированы — детерминизм). Источник `parent_names`
    /// для [`crate::agent::delegate::build_child_registry`] (SUB-3: построение подмножества субагента).
    pub fn names(&self) -> std::collections::BTreeSet<String> {
        self.tools.keys().cloned().collect()
    }

    /// **SUB-3 (security keystone проводки): СУЖАЕТ реестр до `allowed`** — оставляет ТОЛЬКО инструменты,
    /// чьё имя в наборе. Применяется к реестру СУБАГЕНТА после полной сборки (actuator+skills+web), где
    /// `allowed` = [`crate::agent::delegate::build_child_registry`] (child ⊆ parent). Имя НЕ в `allowed`
    /// УДАЛЯЕТСЯ — субагент физически не может вызвать инструмент сверх выданного (эскалация невозможна).
    pub fn retain(&mut self, allowed: &std::collections::BTreeSet<String>) {
        self.tools.retain(|name, _| allowed.contains(name));
    }

    /// Спецификации всех инструментов — для тела запроса к модели (`tools[]`). Порядок не гарантирован
    /// (HashMap); провайдер/модель именами, не позицией.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    /// Диспатчит вызов инструмента → [`ToolResult`]. Неизвестное имя → [`ToolError::UnknownTool`],
    /// зафенсенный как ОШИБОЧНЫЙ результат (НЕ паника, НЕ тихий no-op): модель увидит ошибку и сможет
    /// восстановиться. Любая [`ToolError`] инструмента так же становится `is_error`-результатом.
    pub async fn dispatch(&self, call: &ToolCall) -> ToolResult {
        match self.tools.get(&call.name) {
            None => ToolResult {
                id: call.id.clone(),
                content: ToolError::UnknownTool(call.name.clone()).to_string(),
                is_error: true,
            },
            Some(tool) => match tool.invoke(&call.arguments).await {
                Ok(content) => ToolResult {
                    id: call.id.clone(),
                    content,
                    is_error: false,
                },
                Err(e) => ToolResult {
                    id: call.id.clone(),
                    content: e.to_string(),
                    is_error: true,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::stubs::EchoTool;
    use crate::agent::tool::ToolError;
    use async_trait::async_trait;

    /// Стаб с deny_unknown_fields-аргументами → строгий разбор; кривые args → BadArgs (fail-closed).
    struct StrictTool;

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictArgs {
        n: i64,
    }

    #[async_trait]
    impl Tool for StrictTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "debug.strict".into(),
                description: "строгий разбор аргументов".into(),
                parameters: serde_json::json!({"type":"object"}),
            }
        }
        async fn invoke(&self, args: &str) -> Result<String, ToolError> {
            let parsed: StrictArgs =
                serde_json::from_str(args).map_err(|e| ToolError::BadArgs(e.to_string()))?;
            Ok(format!("n={}", parsed.n))
        }
    }

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args.into(),
        }
    }

    /// Известный инструмент → успешный результат, id сохранён, is_error=false.
    #[tokio::test]
    async fn dispatch_known_returns_result() {
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(EchoTool));
        let res = reg.dispatch(&call("debug.echo", r#"{"text":"hi"}"#)).await;
        assert!(!res.is_error, "echo не ошибка: {res:?}");
        assert_eq!(res.id, "c1");
        assert!(res.content.contains("hi"));
    }

    /// Неизвестное имя → UnknownTool как ОШИБОЧНЫЙ результат (fail-closed, не паника/не no-op).
    #[tokio::test]
    async fn dispatch_unknown_is_failclosed_error() {
        let reg = ToolRegistry::new();
        let res = reg.dispatch(&call("does.not.exist", "{}")).await;
        assert!(res.is_error, "неизвестный инструмент → is_error");
        assert_eq!(res.id, "c1");
        assert!(
            res.content.contains("неизвестный инструмент"),
            "текст несёт причину: {}",
            res.content
        );
    }

    /// Кривые аргументы строгого инструмента → BadArgs как ошибочный результат (не исполнение).
    #[tokio::test]
    async fn dispatch_bad_args_is_failclosed_error() {
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(StrictTool));
        // Неизвестное поле — deny_unknown_fields отвергает.
        let res = reg
            .dispatch(&call("debug.strict", r#"{"n":1,"oops":2}"#))
            .await;
        assert!(res.is_error, "лишнее поле → BadArgs");
        assert!(res.content.contains("аргументы"), "{}", res.content);
        // Валидные args того же инструмента — проходят.
        let ok = reg.dispatch(&call("debug.strict", r#"{"n":7}"#)).await;
        assert!(!ok.is_error);
        assert_eq!(ok.content, "n=7");
    }

    /// specs() перечисляет зарегистрированное; insert под тем же именем вытесняет (last-wins).
    #[test]
    fn specs_and_insert_last_wins() {
        let mut reg = ToolRegistry::new();
        assert!(reg.is_empty());
        reg.insert(Arc::new(EchoTool));
        reg.insert(Arc::new(StrictTool));
        assert_eq!(reg.len(), 2);
        let names: Vec<String> = reg.specs().into_iter().map(|s| s.name).collect();
        assert!(names.contains(&"debug.echo".to_string()));
        assert!(names.contains(&"debug.strict".to_string()));
        // Повторная вставка echo вытесняет прежний (len не растёт).
        let evicted = reg.insert(Arc::new(EchoTool));
        assert!(evicted.is_some());
        assert_eq!(reg.len(), 2);
    }

    /// SUB-3: `names()` отдаёт все имена; `retain` СУЖАЕТ до набора (имя вне набора удаляется) — keystone
    /// проводки субагента (child ⊆ parent). Пустой набор → пустой реестр.
    #[test]
    fn names_and_retain_narrow_to_allowed() {
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(EchoTool)); // debug.echo
        reg.insert(Arc::new(StrictTool)); // debug.strict
        let names = reg.names();
        assert!(names.contains("debug.echo") && names.contains("debug.strict"));

        // retain до {debug.echo} → strict удалён, echo цел.
        let allowed: std::collections::BTreeSet<String> =
            ["debug.echo".to_string()].into_iter().collect();
        reg.retain(&allowed);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.names(), allowed);

        // Пустой набор → пустой реестр.
        reg.retain(&std::collections::BTreeSet::new());
        assert!(reg.is_empty());
    }
}
