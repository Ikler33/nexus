//! Глобальное состояние приложения (Tauri managed state).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{RwLock, RwLockReadGuard};

use crate::ai::{ChatProvider, EmbeddingProvider};
use crate::db::Database;
use crate::error::{AppError, AppResult};
use crate::vector::VectorIndex;

/// Состояние приложения: текущий открытый vault (или его отсутствие).
pub struct AppState {
    /// `None`, пока vault не открыт; `RwLock` — много читателей команд, редкая смена.
    pub vault: RwLock<Option<VaultContext>>,
    /// Флаг отмены активного чат-стрима (UI ведёт один чат за раз). `chat_rag` ставит новый
    /// токен (отменяя предыдущий), `chat_cancel` его взводит. `std::Mutex` — держим коротко, без await.
    pub chat_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// Флаг отмены активного inline-стрима редактора (один inline-запрос за раз, AC-IL-8). Независим
    /// от `chat_cancel`: новый inline-триггер отменяет прежний inline, но НЕ трогает чат (и наоборот).
    pub inline_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// Capability-брокер плагинов (ADR-002, §7.4): токен→сессия + audit. `std::Mutex` — захват только
    /// на синхронную авторизацию (без await; реальный I/O dispatch — после освобождения лока).
    pub plugins: Mutex<crate::plugin::PluginBroker>,
    /// sync-lock git-операций (§8): один синк/коммит за раз. `tokio::Mutex` — держится через `await`
    /// (захват до `spawn_blocking` с libgit2-I/O и до его завершения).
    pub git_lock: tokio::sync::Mutex<()>,
    /// Счётчик активных ИНТЕРАКТИВНЫХ LLM-операций (чат + inline). Планировщик (S5) уступает фоновые
    /// LLM-джобы (дайджест), пока он > 0 — чтобы пользовательский чат/inline не делил локальную модель
    /// с фоном. Инкремент/декремент — RAII-гард [`AppState::enter_interactive_llm`].
    pub interactive_llm: AtomicUsize,
}

/// RAII-гард активной интерактивной LLM-операции: на `Drop` уменьшает счётчик (S5 backpressure).
pub struct InteractiveLlmGuard<'a>(&'a AtomicUsize);
impl Drop for InteractiveLlmGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            vault: RwLock::new(None),
            chat_cancel: Mutex::new(None),
            inline_cancel: Mutex::new(None),
            plugins: Mutex::new(crate::plugin::PluginBroker::new()),
            git_lock: tokio::sync::Mutex::new(()),
            interactive_llm: AtomicUsize::new(0),
        }
    }

    /// Контекст открытого vault — или [`AppError::NoVault`], если vault не открыт (кросс-план #9).
    ///
    /// Единая замена ручному `let g = state.vault.read().await; match g.as_ref() { Some(ctx) => …,
    /// None => return Err(…) }`, разбросанному по командам. Результат — read-гард, спроецированный на
    /// `VaultContext`; read-лок держится, пока жив гард:
    /// - короткая операция → используем напрямую: `let ctx = state.vault().await?; … ctx.db …`;
    /// - долгий `await` (стрим LLM) → клонируем хендлы в блоке и отпускаем гард, чтобы не блокировать
    ///   запись в vault: `let chat = { state.vault().await?.chat.clone() };`.
    ///
    /// Команды, которые при отсутствии vault возвращают значение по умолчанию (а не ошибку), читают
    /// `self.vault` напрямую — для них `?`-семантика не подходит.
    pub async fn vault(&self) -> AppResult<RwLockReadGuard<'_, VaultContext>> {
        let guard = self.vault.read().await;
        RwLockReadGuard::try_map(guard, |o| o.as_ref()).map_err(|_| AppError::NoVault)
    }

    /// Помечает начало интерактивной LLM-операции (чат/inline); гард уменьшит счётчик на `Drop`.
    /// Планировщик уступает фоновые LLM-джобы, пока есть хоть одна (S5).
    pub fn enter_interactive_llm(&self) -> InteractiveLlmGuard<'_> {
        self.interactive_llm.fetch_add(1, Ordering::Relaxed);
        InteractiveLlmGuard(&self.interactive_llm)
    }

    /// Идёт ли сейчас интерактивная LLM-операция (для backpressure планировщика, S5).
    pub fn is_interactive_busy(&self) -> bool {
        self.interactive_llm.load(Ordering::Relaxed) > 0
    }

    /// Взводит флаг отмены текущего чат-стрима (если есть).
    pub fn cancel_active_chat(&self) {
        if let Ok(guard) = self.chat_cancel.lock() {
            if let Some(flag) = guard.as_ref() {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// Регистрирует новый токен отмены для начинающегося чат-стрима, отменив предыдущий.
    pub fn begin_chat(&self) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(false));
        if let Ok(mut guard) = self.chat_cancel.lock() {
            if let Some(prev) = guard.replace(token.clone()) {
                prev.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        token
    }

    /// Взводит флаг отмены текущего inline-стрима редактора (если есть).
    pub fn cancel_active_inline(&self) {
        if let Ok(guard) = self.inline_cancel.lock() {
            if let Some(flag) = guard.as_ref() {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// Регистрирует новый токен отмены для начинающегося inline-стрима, отменив предыдущий (AC-IL-8).
    pub fn begin_inline(&self) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(false));
        if let Ok(mut guard) = self.inline_cancel.lock() {
            if let Some(prev) = guard.replace(token.clone()) {
                prev.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        token
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Контекст открытого vault: корень на диске + его БД + (опц.) RAG-подсистема.
pub struct VaultContext {
    pub root: PathBuf,
    pub db: Database,
    /// Векторный ANN-индекс RAG. `None`, если embedding-провайдер не сконфигурирован
    /// (vault работает и без AI — local-first). Делится с индексатором (пишет) и поиском (читает).
    pub vectors: Option<Arc<VectorIndex>>,
    /// Embedding-провайдер — для эмбеддинга поисковых запросов (Ф1-6) и чат-RAG (Ф1-8).
    /// `None` синхронно с `vectors` (оба есть или обоих нет).
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Chat-провайдер (ADR-005, отдельный хост) — стриминг ответов RAG-чата (Ф1-7).
    /// `None`, если в `local.json` нет `ai.chat`. Независим от embedder.
    pub chat: Option<Arc<dyn ChatProvider>>,
}
