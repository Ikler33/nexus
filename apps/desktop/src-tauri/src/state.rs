//! Глобальное состояние приложения (Tauri managed state).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{RwLock, RwLockReadGuard};

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
    /// Kill-switch «офлайн» эгресса ядра (ADR-005-ext E2/E10, AC-EGR-3): `true` — публичные хосты
    /// отрезаны, LAN/loopback живут. НОВЫЙ атомик, НЕ `chat_cancel` (тот — токен отмены стрима);
    /// `Arc` — тот же флаг читает [`crate::net::EgressPolicy::check`] per-request. Взвод —
    /// ТОЛЬКО через [`AppState::set_egress_offline`] (он же дорезает активный стрим, AC-EGR-11).
    pub egress_offline: Arc<AtomicBool>,
    /// Политика эгресса ядра — ОДИН экземпляр на приложение (AC-EGR-13): из неё строятся все
    /// [`crate::net::GuardedClient`] (open-vault, hot-swap chat, probe настроек).
    pub egress_policy: Arc<crate::net::EgressPolicy>,
    /// Неотключаемый append-only журнал эгресса (E8, AC-EGR-4) — общий для всех guarded-клиентов,
    /// включая probe без открытого vault.
    pub egress_audit: Arc<crate::net::EgressAudit>,
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
        // Дефолт фундамента (E4-трактовка владельца, 2026-06-10): офлайн ВЫКЛЮЧЕН; «облако не из
        // коробки» обеспечивает пустой allowlist политики (явные `ai.*`-хосты кладёт open-vault).
        let egress_offline = Arc::new(AtomicBool::new(false));
        Self {
            vault: RwLock::new(None),
            chat_cancel: Mutex::new(None),
            inline_cancel: Mutex::new(None),
            plugins: Mutex::new(crate::plugin::PluginBroker::new()),
            git_lock: tokio::sync::Mutex::new(()),
            interactive_llm: AtomicUsize::new(0),
            egress_policy: Arc::new(crate::net::EgressPolicy::new(egress_offline.clone())),
            egress_offline,
            egress_audit: Arc::new(crate::net::EgressAudit::default()),
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

    /// Переключает kill-switch «офлайн» эгресса ядра (E2). Включение ДОРЕЗАЕТ активный chat-стрим,
    /// ВЗВОДЯ существующий `chat_cancel` (per-chunk-проверка `cancel.load()` уже есть в `chat.rs`) —
    /// никакого нового механизма отмены (E10, AC-EGR-11).
    pub fn set_egress_offline(&self, offline: bool) {
        self.egress_offline.store(offline, Ordering::Relaxed);
        if offline {
            self.cancel_active_chat();
        }
    }

    /// Снимок политики эгресса для UI настроек и персиста (E5, срез 2).
    pub fn egress_state(&self) -> crate::net::EgressState {
        use crate::net::EgressFeature;
        crate::net::EgressState {
            offline: self.egress_offline.load(Ordering::Relaxed),
            chat: self.egress_policy.is_feature_enabled(EgressFeature::Chat),
            embed: self.egress_policy.is_feature_enabled(EgressFeature::Embed),
            probe: self.egress_policy.is_feature_enabled(EgressFeature::Probe),
        }
    }

    /// Применяет сохранённое состояние политики (загрузка на старте, E5). Идёт через
    /// [`Self::set_egress_offline`] — включённый офлайн дорежет активный стрим и здесь
    /// (на старте его нет, но инвариант един).
    pub fn apply_egress_state(&self, s: &crate::net::EgressState) {
        use crate::net::EgressFeature;
        self.set_egress_offline(s.offline);
        self.egress_policy
            .set_feature_enabled(EgressFeature::Chat, s.chat);
        self.egress_policy
            .set_feature_enabled(EgressFeature::Embed, s.embed);
        self.egress_policy
            .set_feature_enabled(EgressFeature::Probe, s.probe);
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
    /// Фасад AI-подсистемы (§4.3, AC-EGR-13): все провайдеры (chat/chat_fast/chat_util/embedder)
    /// плюс политика эгресса одним полем — вместо четырёх независимых `Arc`. Провайдеры ходят в
    /// сеть ТОЛЬКО через `net::GuardedClient` (ADR-005-ext).
    pub ai: crate::ai::AIClient,
    /// Реестр зарегистрированных HOME-виджетов (H2): по нему `refresh_widget` проверяет, что ключ
    /// известен, прежде чем ставить джобу. Наполняется в `open_vault` (H3+ регистрируют виджеты);
    /// сейчас пуст. `Arc` — делится между командами без клонирования множества.
    pub widgets: Arc<crate::home::widgets::WidgetRegistry>,
    /// Управляющий вход watcher-петли индексатора: команда `rescan_vault` шлёт
    /// [`crate::watcher::VaultEvent::Rescan`] (ручной реиндекс сериализуется с fs-событиями в
    /// одной петле). `None` — watcher не инициализировался (vault без живой индексации).
    pub index_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::watcher::VaultEvent>>,
    /// Якорь фоновых воркеров vault (фикс «вечных воркеров», аудит 2026-06-10): пока жив контекст —
    /// живут watcher-петля индексатора и воркер планировщика; замена/сброс контекста дропает якорь
    /// → петли гаснут сами. Раньше каждый `open_vault` плодил ЕЩЁ один вечный воркер (двойная
    /// индексация, LLM-джобы закрытого vault продолжали жечь модель).
    pub lifecycle: VaultLifecycle,
}

/// Хендлы жизненного цикла фоновых задач vault (см. [`VaultContext::lifecycle`]). Поля нигде не
/// читаются — их работа выполняется на `Drop`: watcher закрывает mpsc-канал петли индексатора,
/// watch-sender завершает `scheduler::worker_loop` (`changed()` → `Err` → break).
pub struct VaultLifecycle {
    /// FS-watcher vault: дроп → sender событий закрыт → петля индексации выходит.
    pub watcher: Option<crate::watcher::VaultWatcher>,
    /// Shutdown-канал воркера планировщика: дроп sender'а гасит цикл.
    pub scheduler_shutdown: tokio::sync::watch::Sender<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{EgressDenied, EgressFeature};

    /// AC-EGR-11 (E10): включение «офлайн» рвёт активный chat-стрим, ВЗВОДЯ существующий
    /// `chat_cancel`-токен — никакого нового механизма отмены. Политика видит тот же атомик.
    #[test]
    fn egress_offline_arms_existing_chat_cancel() {
        let state = AppState::new();
        let token = state.begin_chat();
        assert!(!token.load(Ordering::Relaxed), "новый стрим не отменён");

        state.set_egress_offline(true);
        assert!(
            token.load(Ordering::Relaxed),
            "«офлайн» взводит СУЩЕСТВУЮЩИЙ chat_cancel (AC-EGR-11)"
        );
        // Тот же флаг читает политика per-request: публичный хост → Offline, loopback живёт (E2).
        assert_eq!(
            state
                .egress_policy
                .check("203.0.113.7", EgressFeature::Chat),
            Err(EgressDenied::Offline)
        );
        assert_eq!(
            state.egress_policy.check("127.0.0.1", EgressFeature::Chat),
            Ok(())
        );

        state.set_egress_offline(false);
        assert!(
            token.load(Ordering::Relaxed),
            "выключение офлайна не «развзводит» уже отменённый стрим"
        );
        assert_eq!(
            state.egress_policy.check("127.0.0.1", EgressFeature::Chat),
            Ok(())
        );
    }
}
