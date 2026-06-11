//! Agent-loop web-агента (W-2): LLM решает «нужен ли интернет» → поисковый запрос → поиск.
//! Оркестрация отделена от чат-команды и работает с трейтами [`Searcher`]/[`ChatProvider`] —
//! тестируется с моками, без SearXNG/LLM-сервера.
//!
//! **Лимит W3:** не больше [`MAX_SEARCHES`] поисков на один ход чата (анти-runaway-агент). v1
//! планирует ОДИН запрос; константа — жёсткий потолок, не молчаливый кап (превышение невозможно
//! by construction, но граница названа явно). **tool-use в web-агенте запрещён** (ADR W-аддендум):
//! результаты идут только как недоверенный контекст для ответа, модель не вызывает инструменты.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::ai::{build_web_query_messages, parse_web_query_plan, ChatProvider};

use super::search::{SearchError, SearchResult, Searcher};

/// W3: максимум web-поисков на один ход чата (анти-runaway).
pub const MAX_SEARCHES: usize = 3;

/// Итог планирования+поиска. `query=None` → интернет не нужен (ответит общий чат).
#[derive(Debug, Default)]
pub struct WebAgentOutcome {
    /// Поисковый запрос, который выбрала модель (`None` — веб не нужен).
    pub query: Option<String>,
    /// Найденные результаты (пусто при `query=None` или нулевой выдаче).
    pub results: Vec<SearchResult>,
}

/// Шаг 1: спрашиваем планировщик (мелкая модель), нужен ли веб, и какой запрос. Не-стрим: собираем
/// ответ в строку. Ошибка/отмена планировщика → `None` (деградируем к общему чату, не падаем).
async fn plan_query(
    planner: &dyn ChatProvider,
    question: &str,
    cancel: &Arc<AtomicBool>,
) -> Option<String> {
    let messages = build_web_query_messages(question);
    let mut out = String::new();
    let mut sink = |t: String| out.push_str(&t);
    match planner.stream_chat(&messages, &mut sink, cancel).await {
        Ok(_) => parse_web_query_plan(&out),
        Err(_) => None,
    }
}

/// Полный шаг web-агента: план → (если нужен веб) поиск. `planner` — мелкая модель для решения,
/// `searcher` — web-поиск. Возвращает запрос+результаты; W4 (секрет в запросе) пробрасывается
/// типизированно, чтобы команда показала «запрос не отправлен».
pub async fn run(
    planner: &dyn ChatProvider,
    searcher: &dyn Searcher,
    question: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<WebAgentOutcome, SearchError> {
    let Some(query) = plan_query(planner, question, cancel).await else {
        return Ok(WebAgentOutcome::default()); // веб не нужен
    };
    // v1: один запрос (≤ MAX_SEARCHES — потолок назван явно в константе, W3).
    let results = searcher.search(&query).await?;
    Ok(WebAgentOutcome {
        query: Some(query),
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{AiResult, ChatMessage};
    use std::sync::atomic::Ordering;

    /// Планировщик-мок: отдаёт заранее заданную строку плана.
    struct PlannerMock(&'static str);
    #[async_trait::async_trait]
    impl ChatProvider for PlannerMock {
        async fn stream_chat(
            &self,
            _m: &[ChatMessage],
            on_token: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
        ) -> AiResult<String> {
            on_token(self.0.to_string());
            Ok(self.0.to_string())
        }
        fn model_id(&self) -> &str {
            "planner-mock"
        }
    }

    /// Поисковик-мок: считает вызовы, отдаёт фиксированную выдачу.
    struct SearcherMock {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        results: Vec<SearchResult>,
    }
    #[async_trait::async_trait]
    impl Searcher for SearcherMock {
        async fn search(&self, _q: &str) -> Result<Vec<SearchResult>, SearchError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.results.clone())
        }
    }

    fn cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn results() -> Vec<SearchResult> {
        vec![SearchResult {
            title: "T".into(),
            url: "https://x.test".into(),
            snippet: "s".into(),
        }]
    }

    #[tokio::test]
    async fn plan_none_skips_search() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let searcher = SearcherMock {
            calls: calls.clone(),
            results: results(),
        };
        let out = run(&PlannerMock("NONE"), &searcher, "сколько 2+2", &cancel())
            .await
            .unwrap();
        assert!(out.query.is_none());
        assert!(out.results.is_empty());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "веб не нужен → поиск не зван"
        );
    }

    #[tokio::test]
    async fn plan_query_runs_single_search() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let searcher = SearcherMock {
            calls: calls.clone(),
            results: results(),
        };
        let out = run(
            &PlannerMock("react 19 release date"),
            &searcher,
            "когда вышел react 19",
            &cancel(),
        )
        .await
        .unwrap();
        assert_eq!(out.query.as_deref(), Some("react 19 release date"));
        assert_eq!(out.results.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "≤ MAX_SEARCHES (W3)");
    }

    #[tokio::test]
    async fn search_error_propagates_typed() {
        struct FailSearcher;
        #[async_trait::async_trait]
        impl Searcher for FailSearcher {
            async fn search(&self, _q: &str) -> Result<Vec<SearchResult>, SearchError> {
                Err(SearchError::SecretInQuery)
            }
        }
        let err = run(&PlannerMock("q"), &FailSearcher, "вопрос", &cancel()).await;
        assert!(matches!(err, Err(SearchError::SecretInQuery)));
    }
}
