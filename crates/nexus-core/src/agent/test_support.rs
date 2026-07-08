//! Общие тест-хелперы агентного дерева (R-13g). `#[cfg(test)]`-only, `pub(crate)`.
//!
//! Канон для примитивов, которые исторически копировались по `#[cfg(test)] mod tests` разных модулей
//! `agent::*`. Сюда сведено ТОЛЬКО байт-идентичное; специализированные варианты (напр. FakeProvider с
//! записью `seen_tools` в `session::tests` или с эмиссией токена на `Final` в `connect::acp::server`)
//! ОСТАВЛЕНЫ по месту — их поведение отличается, слияние запутало бы тесты.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tempfile::TempDir;

use crate::agent::tool::ToolSpec;
use crate::ai::tools::{ToolCapableProvider, ToolTurn};
use crate::ai::{AiResult, ChatMessage};
use crate::db::Database;
use crate::net::RunCtx;

/// Открывает временную БД для тестов: `(TempDir, Database)` на `<tmp>/test.db`.
///
/// Канон (R-13g): байт-идентичные копии жили в `connect::handler`, `connect::client`,
/// `connect::acp::server::tests`, `session::tests`. `TempDir` возвращается вызывающему — держит
/// каталог живым на время теста.
pub(crate) async fn open_db() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path().join("test.db")).await.unwrap();
    (dir, db)
}

/// Скриптованный fake tool-провайдер: FIFO заданных ходов, БЕЗ побочек (не пишет `seen_tools`, не
/// эмитит токен на `Final`). Канон (R-13g): байт-идентичные копии жили в `connect::client` и
/// `connect::handler`. Специализированные варианты (`session::tests` со `seen_tools`,
/// `connect::acp::server` с эмиссией токена) — ОТДЕЛЬНЫЕ, по месту.
///
/// Конструируется `FakeProvider::new(turns)` → `Self`; вызывающий сам оборачивает в `Arc`.
pub(crate) struct FakeProvider {
    turns: Mutex<VecDeque<AiResult<ToolTurn>>>,
}

impl FakeProvider {
    pub(crate) fn new(turns: Vec<AiResult<ToolTurn>>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
        }
    }
}

#[async_trait]
impl ToolCapableProvider for FakeProvider {
    async fn stream_chat_tools(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ToolSpec],
        _on_token: &mut (dyn FnMut(String) + Send),
        _cancel: &Arc<AtomicBool>,
        _ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(ToolTurn::Final("(no more turns)".into())))
    }
    fn model_id(&self) -> &str {
        "fake"
    }
}
