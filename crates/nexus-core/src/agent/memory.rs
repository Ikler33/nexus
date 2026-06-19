//! [`AgentMemory`] — мост агента к ТРЁМ продакшн-слоям памяти Nexus (AGENT-MEM-1, Фаза 1).
//!
//! Цикл агента (AGENT-1/2) до этого среза стартовал с «голым» контекстом `[системный преамбул,
//! задача]`. AGENT-MEM-1 даёт ему ту же память, что инжектится в обычный чат-ответ ИИ:
//! - **факты (MEM)** — курируемые ЯВНЫЕ факты о пользователе (`memory::context_facts`, пины + top-k);
//! - **переписка (N4b)** — семантически близкие фрагменты прошлых диалогов (`chat_log::search_memory`);
//! - **эпизоды (EP)** — краткие саммари завершённых сессий (`episode::search_episodes`).
//!
//! # Два метода, две гарантии
//! - [`AgentMemory::recall`] — **только чтение**: собирает фенсенный контекст из 3 слоёв в рамках
//!   токен-бюджета и возвращает его как `ChatMessage`-ы (роль `user` — это ДАННЫЕ, не инструкции,
//!   I-5). Деградирует тихо: нет эмбеддера/индексов → пустой `Vec`, прогон не падает.
//! - [`AgentMemory::remember`] — **только запись Add** (`memory::add`, source="agent"). НИКОГДА не
//!   update/delete: автономная консолидация (слияние/супридирование/удаление) — это ГЕЙТЕД эпик MEM
//!   (`memory/consolidate.rs`), вне скоупа автономного агента. Воронка Add-only стережётся
//!   grep-линтом `scripts/check-agent-memory.mjs`: `memory::add` в модуле `agent/` разрешён ТОЛЬКО
//!   в ЭТОМ файле — цикл/хендлер/инструменты пишут память ИСКЛЮЧИТЕЛЬНО через `remember`.
//!
//! # Исключение текущей сессии (анти-self-leak)
//! [`VaultAgentMemory`] хранит `exclude_session` (id chat-сессии, привязанной к прогону) и передаёт
//! его во ВСЕ три поиска: чат/эпизоды через `exclude_session`, факты не сессийны (исключать нечего).
//! Так прогон не «вспоминает» собственную, ещё идущую сессию. Сегодня `agent_runs.session_id` всегда
//! NULL (линковка прогон↔chat-сессия — поздний срез), поэтому exclude обычно `None` — и это
//! корректно: прогон агента пишет в `agent_runs`/`memory_facts`, НЕ в `chat_messages`/`chat_episodes`,
//! так что протечь его «текущей сессии» в N4b/EP физически нечему. Канал проброса готов на будущее.
//!
//! # Мокабельность
//! Трейт объект-безопасен и async (`async_trait`), БЕЗ зависимости от БД/usearch/эмбеддера/LLM —
//! тестовый [`MockAgentMemory`] (под `cfg(test)`/`test-util`) отдаёт канонический recall и пишет
//! вызовы `remember` в вектор, чтобы тесты хендлера не таскали реальный стек памяти.

use std::sync::Arc;

use async_trait::async_trait;

use crate::ai::{
    build_agent_memory_block, build_episode_block, build_memory_block, injection_marker,
    ChatMessage, ContextBudget, EmbeddingProvider, QwenTokenizer,
};
use crate::chunker::Tokenizer;
use crate::db::{ReadPool, WriteActor};
use crate::vector::VectorIndex;

/// Источник факта, записанного автономным агентом (колонка `memory_facts.source`). Отдельная метка
/// от `explicit` (курировал пользователь) / `auto` (предложила консолидация): видно, что факт
/// положил агент в ходе прогона.
pub const SOURCE_AGENT: &str = "agent";

/// Сколько фактов максимум подмешивать в recall (top-k не-пинов; пины добавляются сверх и капятся
/// в `memory::context_facts`). Скромный k — память агента это ФОН, а не основной материал прогона.
const RECALL_FACTS_K: usize = 5;
/// Сколько фрагментов переписки (N4b) максимум подмешивать (дедуп по сессии — один лучший на диалог).
const RECALL_CHAT_K: usize = 3;
/// Сколько эпизодов (саммари сессий) максимум подмешивать.
const RECALL_EPISODE_K: usize = 2;
/// Длина выжимки фрагмента переписки/эпизода (символы) — как в чат-инъекции (build_memory_block
/// усечения не делает, режем на входе).
const RECALL_SNIPPET_CHARS: usize = 400;

/// Мост агента к памяти Nexus: recall (чтение 3 слоёв) + remember (запись Add-only). Объект-безопасен
/// и мокабелен (см. модульный док). `Arc<dyn AgentMemory>` держит [`crate::agent::AgentRunHandler`].
#[async_trait]
pub trait AgentMemory: Send + Sync {
    /// Собирает фенсенный контекст памяти под запрос `query` в рамках токен-бюджета `budget`
    /// (число токенов, отведённых ПОД ПАМЯТЬ в начальном контексте прогона). Возвращает
    /// `ChatMessage`-ы роли `user` (ДАННЫЕ, не инструкции — I-5), готовые встать МЕЖДУ системным
    /// преамбулом и задачей. Пустой `Vec` — памяти нет / слой деградировал (НИКОГДА не ошибка:
    /// отсутствие памяти не валит прогон).
    async fn recall(&self, query: &str, budget: usize) -> Vec<ChatMessage>;

    /// Автономная запись факта в память — **только Add** (`memory::add`, дедуп по точному тексту).
    /// Возвращает `Ok(is_new)`: `true` — создана НОВАЯ строка, `false` — точный дубль (id уже был).
    /// `Err` — инфраструктурный сбой записи. НИКОГДА не update/delete (консолидация — гейтед MEM).
    async fn remember(&self, text: &str) -> Result<bool, String>;
}

/// Продакшн-адаптер [`AgentMemory`] поверх трёх vault-индексов + БД (AGENT-MEM-1).
///
/// Держит долю `ReadPool`/`WriteActor` (чтение слоёв / запись Add), эмбеддер и ТРИ `VectorIndex`
/// (факты/переписка/эпизоды) + `exclude_session` прогона. Любой из индексов/эмбеддер `None` →
/// соответствующий слой recall'а просто пуст (graceful degrade, см. [`AgentMemory::recall`]).
#[derive(Clone)]
pub struct VaultAgentMemory {
    reader: ReadPool,
    writer: WriteActor,
    /// Эмбеддер запросов. `None` → recall не ищет ни в одном слое (фактов/переписки/эпизодов),
    /// возвращает пусто; remember (Add в БД) при этом всё равно работает (эмбеддинг ему не нужен).
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Индекс фактов (MEM, ключ = `memory_facts.id`). `None` → слой фактов в recall пуст (но пины
    /// всё равно НЕ инжектятся: без индекса `context_facts` тоже отдаёт только пины — мы их НЕ
    /// зовём при отсутствии индекса, чтобы recall был единообразно «памяти нет»).
    facts_vectors: Option<Arc<VectorIndex>>,
    /// Индекс переписки (N4b, ключ = id сообщения). `None` → слой переписки пуст.
    chat_vectors: Option<Arc<VectorIndex>>,
    /// Индекс эпизодов (EP, ключ = `chat_episodes.id`). `None` → слой эпизодов пуст.
    episode_vectors: Option<Arc<VectorIndex>>,
    /// Id chat-сессии прогона для исключения из recall (анти-self-leak). `None` сегодня (линковка
    /// прогон↔сессия — поздний срез); канал проброса готов.
    exclude_session: Option<i64>,
}

impl VaultAgentMemory {
    /// Собирает адаптер из vault-зависимостей. Любой `None`-индекс / `None`-эмбеддер делает
    /// соответствующий слой recall'а пустым (degrade-safe). `exclude_session` — id chat-сессии
    /// прогона (анти-self-leak), `None` если прогон не привязан к сессии.
    pub fn new(
        reader: ReadPool,
        writer: WriteActor,
        embedder: Option<Arc<dyn EmbeddingProvider>>,
        facts_vectors: Option<Arc<VectorIndex>>,
        chat_vectors: Option<Arc<VectorIndex>>,
        episode_vectors: Option<Arc<VectorIndex>>,
        exclude_session: Option<i64>,
    ) -> Self {
        Self {
            reader,
            writer,
            embedder,
            facts_vectors,
            chat_vectors,
            episode_vectors,
            exclude_session,
        }
    }

    /// Слой ФАКТОВ (MEM): top-k близких + пины из `memory::context_facts`, обёрнутые
    /// `build_agent_memory_block`. Пусто (`None`), если нет эмбеддера/индекса или фактов не нашлось.
    async fn recall_facts(&self, query: &str, marker: &str) -> Option<ChatMessage> {
        let (embedder, vectors) = (self.embedder.as_ref()?, self.facts_vectors.as_ref()?);
        let facts = crate::memory::context_facts(
            &self.reader,
            &self.writer,
            vectors,
            embedder.as_ref(),
            query,
            RECALL_FACTS_K,
        )
        .await
        .map_err(|e| tracing::warn!(error = %e, "agent recall: слой фактов деградирует"))
        .ok()?;
        // Метка вида источника (не несёт доверия — внутри маркеров): порядковый номер факта.
        let pairs: Vec<(String, String)> = facts
            .into_iter()
            .enumerate()
            .map(|(i, f)| (format!("факт #{}", i + 1), f.text))
            .collect();
        build_agent_memory_block(&pairs, marker).map(ChatMessage::user)
    }

    /// Слой ПЕРЕПИСКИ (N4b): семантически близкие фрагменты прошлых диалогов из
    /// `chat_log::search_memory`, обёрнутые `build_memory_block`. Исключает текущую сессию прогона.
    async fn recall_chat(&self, query: &str, marker: &str) -> Option<ChatMessage> {
        let (embedder, vectors) = (self.embedder.as_ref()?, self.chat_vectors.as_ref()?);
        let hits = crate::chat_log::search_memory(
            &self.reader,
            vectors,
            embedder.as_ref(),
            query,
            RECALL_CHAT_K,
            self.exclude_session,
            std::collections::HashSet::new(),
            RECALL_SNIPPET_CHARS,
        )
        .await
        .map_err(|e| tracing::warn!(error = %e, "agent recall: слой переписки деградирует"))
        .ok()?;
        let pairs: Vec<(String, String)> = hits
            .into_iter()
            .map(|h| {
                (
                    format!("разговор «{}» ({})", h.session_title, h.role),
                    h.snippet,
                )
            })
            .collect();
        build_memory_block(&pairs, marker).map(ChatMessage::user)
    }

    /// Слой ЭПИЗОДОВ (EP): краткие саммари завершённых сессий из `episode::search_episodes`,
    /// обёрнутые `build_episode_block`. Исключает текущую сессию прогона.
    async fn recall_episodes(&self, query: &str, marker: &str) -> Option<ChatMessage> {
        let (embedder, vectors) = (self.embedder.as_ref()?, self.episode_vectors.as_ref()?);
        let hits = crate::episode::search_episodes(
            &self.reader,
            vectors,
            embedder.as_ref(),
            query,
            RECALL_EPISODE_K,
            self.exclude_session,
            RECALL_SNIPPET_CHARS,
        )
        .await
        .map_err(|e| tracing::warn!(error = %e, "agent recall: слой эпизодов деградирует"))
        .ok()?;
        let pairs: Vec<(String, String)> = hits
            .into_iter()
            .map(|h| (format!("эпизод «{}»", h.session_title), h.summary_snippet))
            .collect();
        build_episode_block(&pairs, marker).map(ChatMessage::user)
    }
}

/// Укладывает собранные слои в `budget` токенов, ДРОПАЯ слои наименьшего приоритета первыми.
///
/// `layers` приходят в порядке ВОЗРАСТАНИЯ приоритета (первый — наименее ценный): мы пытаемся
/// уместить хвост (самые ценные), отбрасывая голову при переполнении. Каждый слой считается целиком
/// (атомарный блок — частичный фенсенный блок резать нельзя, он бы порвал маркеры). Бюджет меряется
/// той же `ContextBudget::message_cost`, что и весь цикл (один источник cost-математики). Если даже
/// один самый ценный слой не влезает в `budget` — он отбрасывается (recall не имеет права раздуть
/// окно сверх отведённого ему куска; цикл/`fit` потом всё равно подрежет, но честнее не превышать
/// здесь). Результат — в ИСХОДНОМ порядке приоритета (факты последними — ближе всего к задаче).
fn pack_within_budget(
    tk: &dyn Tokenizer,
    layers: Vec<ChatMessage>,
    budget: usize,
) -> Vec<ChatMessage> {
    let mut used = 0usize;
    // Идём С КОНЦА (приоритетные первыми): какие индексы оставить.
    let mut keep: Vec<usize> = Vec::new();
    for (i, m) in layers.iter().enumerate().rev() {
        let cost = ContextBudget::message_cost(tk, m);
        if used + cost <= budget {
            used += cost;
            keep.push(i);
        }
        // НЕ break: более дешёвый менее-приоритетный слой мог бы влезть в остаток. Но порядок
        // приоритета сохраняется фильтром ниже (мы не переставляем, только выкидываем).
    }
    let keep: std::collections::HashSet<usize> = keep.into_iter().collect();
    layers
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep.contains(i))
        .map(|(_, m)| m)
        .collect()
}

#[async_trait]
impl AgentMemory for VaultAgentMemory {
    async fn recall(&self, query: &str, budget: usize) -> Vec<ChatMessage> {
        if query.trim().is_empty() || budget == 0 {
            return Vec::new();
        }
        // Per-request маркер фенсинга (неугадываемый): один на весь recall — все блоки этого прогона
        // обёрнуты ОДНИМ маркером, как и в чат-инъекции. Анти-инъекция I-5/AC-SEC-7.
        let marker = injection_marker();
        // Слои собираем независимо (любой может деградировать в None). Порядок ВОЗРАСТАНИЯ приоритета:
        // переписка (сырые реплики) < эпизоды (саммари) < факты (курируемые явные факты, ближе к задаче).
        // При нехватке бюджета первой выпадает переписка, последними — факты.
        let layers: Vec<ChatMessage> = [
            self.recall_chat(query, &marker).await,
            self.recall_episodes(query, &marker).await,
            self.recall_facts(query, &marker).await,
        ]
        .into_iter()
        .flatten()
        .collect();
        if layers.is_empty() {
            return Vec::new();
        }
        // Бюджет меряем тем же токенайзером, что цикл агента (встроенный Qwen — реальная модель).
        let tk = QwenTokenizer::embedded();
        pack_within_budget(&tk, layers, budget)
    }

    async fn remember(&self, text: &str) -> Result<bool, String> {
        // Add-only: единственная точка записи памяти агента. `memory::add` дедупит по точному тексту
        // (UNIQUE) — возвращает (id, is_new). Пустой текст → None (нечего записывать → не новое).
        match crate::memory::add(&self.writer, text, SOURCE_AGENT).await {
            Ok(Some((_id, is_new))) => Ok(is_new),
            Ok(None) => Ok(false), // пустой/whitespace — не записан, не «новое»
            Err(e) => Err(format!("agent remember (Add): {e}")),
        }
    }
}

/// Мок [`AgentMemory`] для тестов хендлера/цикла: канонический recall + журнал вызовов remember.
/// БЕЗ БД/usearch/эмбеддера/LLM — доказывает мокабельность трейта (модульный док).
#[cfg(any(test, feature = "test-util"))]
pub struct MockAgentMemory {
    /// Что вернёт `recall` (как есть, без учёта бюджета — мок проверяет ПРОВОДКУ, не упаковку).
    pub canned: Vec<ChatMessage>,
    /// Журнал текстов, переданных в `remember` (для assert'ов теста воронки).
    pub remembered: std::sync::Mutex<Vec<String>>,
    /// Что вернёт `remember` (is_new). По умолчанию `true`.
    pub remember_returns: bool,
}

#[cfg(any(test, feature = "test-util"))]
impl MockAgentMemory {
    /// Мок с заданным каноническим recall'ом (remember → Ok(true), журнал пуст).
    pub fn with_canned(canned: Vec<ChatMessage>) -> Self {
        Self {
            canned,
            remembered: std::sync::Mutex::new(Vec::new()),
            remember_returns: true,
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
#[async_trait]
impl AgentMemory for MockAgentMemory {
    async fn recall(&self, _query: &str, _budget: usize) -> Vec<ChatMessage> {
        self.canned.clone()
    }
    async fn remember(&self, text: &str) -> Result<bool, String> {
        self.remembered.lock().unwrap().push(text.to_string());
        Ok(self.remember_returns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::MockEmbedder;
    use crate::db::Database;
    use rusqlite::params;
    use tempfile::TempDir;

    const DIM: usize = 16;

    async fn open() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        (dir, db)
    }

    fn open_index(dir: &TempDir, name: &str) -> Arc<VectorIndex> {
        Arc::new(VectorIndex::open(dir.path().join(name), DIM).unwrap())
    }

    /// Сеет факт в БД + его вектор в индекс (как делает командный путь add→index_fact).
    async fn seed_fact(db: &Database, idx: &VectorIndex, emb: &MockEmbedder, text: &str) -> i64 {
        let (id, _) = crate::memory::add(db.writer(), text, "explicit")
            .await
            .unwrap()
            .unwrap();
        crate::memory::index_fact(idx, emb, id, text).await.unwrap();
        id
    }

    /// Сеет chat-сессию с одним user-сообщением + его вектор в chat-индекс. Возвращает (session_id).
    async fn seed_chat(db: &Database, idx: &VectorIndex, emb: &MockEmbedder, text: &str) -> i64 {
        let t = text.to_string();
        let (sid, msg_id) = db
            .writer()
            .transaction(move |tx| {
                let ts = 1_000_000i64;
                tx.execute(
                    "INSERT INTO chat_sessions(title, created_at, updated_at) VALUES(?1, ?2, ?2)",
                    params![t, ts],
                )?;
                let sid = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO chat_messages(session_id, role, content, sources_json, created_at) \
                     VALUES(?1, 'user', ?2, NULL, ?3)",
                    params![sid, t, ts],
                )?;
                Ok((sid, tx.last_insert_rowid()))
            })
            .await
            .unwrap();
        let v = emb.embed_documents(&[text]).await.unwrap();
        idx.upsert(msg_id as u64, &v[0]).unwrap();
        sid
    }

    /// Сеет эпизод (сессия + строка chat_episodes) + его вектор в episode-индекс. Возвращает session_id.
    async fn seed_episode(
        db: &Database,
        idx: &VectorIndex,
        emb: &MockEmbedder,
        summary: &str,
    ) -> i64 {
        let s = summary.to_string();
        let (sid, eid) = db
            .writer()
            .transaction(move |tx| {
                let ts = 1_000_000i64;
                tx.execute(
                    "INSERT INTO chat_sessions(title, created_at, updated_at) VALUES(?1, ?2, ?2)",
                    params![s, ts],
                )?;
                let sid = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO chat_episodes(session_id, summary, topics, msg_count, last_msg_id, \
                       started_at, ended_at, model, embed_model, generated_at, dismissed) \
                     VALUES(?1, ?2, NULL, 4, 10, ?3, ?3, 'm', 'mock', ?3, 0)",
                    params![sid, s, ts],
                )?;
                Ok((sid, tx.last_insert_rowid()))
            })
            .await
            .unwrap();
        let v = emb.embed_documents(&[summary]).await.unwrap();
        idx.upsert(eid as u64, &v[0]).unwrap();
        sid
    }

    /// AGENT-MEM-1: recall собирает фенсенные блоки из ВСЕХ ТРЁХ слоёв (факты/переписка/эпизоды) —
    /// каждый обёрнут per-request маркером, текст по семантически близкому запросу найден.
    #[tokio::test]
    async fn recall_assembles_three_layers_fenced() {
        let (dir, db) = open().await;
        let emb = MockEmbedder { dim: DIM };
        let fi = open_index(&dir, "f.usearch");
        let ci = open_index(&dir, "c.usearch");
        let ei = open_index(&dir, "e.usearch");

        let query = "проект про векторный поиск заметок";
        seed_fact(&db, &fi, &emb, query).await;
        seed_chat(&db, &ci, &emb, query).await;
        seed_episode(&db, &ei, &emb, query).await;

        let mem = VaultAgentMemory::new(
            db.reader().clone(),
            db.writer().clone(),
            Some(Arc::new(emb)),
            Some(fi),
            Some(ci),
            Some(ei),
            None,
        );
        // Щедрый бюджет — все три слоя влезают.
        let msgs = mem.recall(query, 4096).await;
        assert_eq!(msgs.len(), 3, "три слоя собраны: {msgs:?}");
        for m in &msgs {
            assert_eq!(m.role, "user", "память — ДАННЫЕ роли user (I-5), не system");
            // Каждый блок обёрнут маркером фенсинга (⟦…⟧) на обоих концах.
            assert!(
                m.content.contains('⟦'),
                "блок фенсен маркером: {}",
                m.content
            );
        }
        // Контент слоёв узнаётся по преамбулам блоков.
        let joined = msgs.iter().map(|m| m.content.as_str()).collect::<String>();
        assert!(joined.contains("сохранённые факты"), "слой фактов");
        assert!(
            joined.contains("Память прошлых разговоров"),
            "слой переписки"
        );
        assert!(
            joined.contains("Эпизоды прошлых разговоров"),
            "слой эпизодов"
        );
    }

    /// AGENT-MEM-1: recall ИСКЛЮЧАЕТ текущую сессию прогона — близкий по тексту фрагмент переписки/
    /// эпизод СОБСТВЕННОЙ сессии не подмешивается (анти-self-leak).
    #[tokio::test]
    async fn recall_excludes_current_session() {
        let (dir, db) = open().await;
        let emb = MockEmbedder { dim: DIM };
        let ci = open_index(&dir, "c.usearch");
        let ei = open_index(&dir, "e.usearch");

        let query = "разговор про настройку SearXNG на VPS";
        // Сессия переписки и эпизод — ОБА это текущая сессия прогона (её и исключаем).
        let chat_sid = seed_chat(&db, &ci, &emb, query).await;
        let ep_sid = seed_episode(&db, &ei, &emb, query).await;
        assert_ne!(chat_sid, ep_sid);

        // Память с exclude_session = сессия переписки: её фрагмент выпадает; эпизод другой сессии —
        // остаётся (его сессия != exclude). Проверяем обе стороны разом, исключая chat_sid.
        let mem = VaultAgentMemory::new(
            db.reader().clone(),
            db.writer().clone(),
            Some(Arc::new(MockEmbedder { dim: DIM })),
            None,
            Some(ci.clone()),
            Some(ei.clone()),
            Some(chat_sid),
        );
        let msgs = mem.recall(query, 4096).await;
        let joined = msgs.iter().map(|m| m.content.as_str()).collect::<String>();
        assert!(
            !joined.contains("Память прошлых разговоров"),
            "переписка текущей сессии исключена: {joined}"
        );
        assert!(
            joined.contains("Эпизоды прошлых разговоров"),
            "эпизод ДРУГОЙ сессии остаётся: {joined}"
        );

        // Контр-проба: исключаем сессию ЭПИЗОДА — теперь выпадает эпизод, переписка остаётся.
        let mem2 = VaultAgentMemory::new(
            db.reader().clone(),
            db.writer().clone(),
            Some(Arc::new(MockEmbedder { dim: DIM })),
            None,
            Some(ci),
            Some(ei),
            Some(ep_sid),
        );
        let joined2 = mem2
            .recall(query, 4096)
            .await
            .iter()
            .map(|m| m.content.as_str())
            .collect::<String>();
        assert!(
            !joined2.contains("Эпизоды прошлых разговоров"),
            "эпизод текущей сессии исключён: {joined2}"
        );
        assert!(
            joined2.contains("Память прошлых разговоров"),
            "переписка ДРУГОЙ сессии остаётся: {joined2}"
        );
    }

    /// AGENT-MEM-1: при недостатке бюджета recall ДРОПАЕТ слой наименьшего приоритета первым
    /// (переписка раньше эпизодов раньше фактов). Бюджет, в который влезают только факты, оставляет
    /// факты.
    ///
    /// # ДЕТЕРМИНИЗМ (AGENT-5 flake-fix)
    /// Раньше бюджет был `facts_cost + 16` при ПОЧТИ РАВНЫХ по стоимости слоях (все три засеяны одним
    /// коротким `query`). Это сажало решение о дропе ПРЯМО НА ГРАНИЦУ: `recall` пересоздаёт
    /// per-request `injection_marker` (24-символьный hex, дважды на блок), и BPE-токенизация нового
    /// маркера в `tight` могла отличаться от меренной по `full` на ±несколько токенов — то факты не
    /// влезали (`tight.len()==0`), то рядом протискивался ещё и почти-такой-же дешёвый эпизод
    /// (`tight.len()==2`). Тест флакал на маркер-дрейфе.
    ///
    /// Теперь граница НЕДВУСМЫСЛЕННА за счёт КРУПНОГО РАЗРЫВА стоимостей: переписка/эпизоды засеяны
    /// длинными телами (сотни токенов каждое), факты — короткие. Бюджет = `facts_cost + GAP`, где
    /// `GAP` (64 токена) КРАТНО больше любого маркер-дрейфа (несколько токенов), но КРАТНО меньше
    /// стоимости любого из менее приоритетных слоёв. Поэтому: факты ВСЕГДА влезают (margin >> дрейф),
    /// а переписка/эпизоды ГАРАНТИРОВАННО нет (их стоимость >> бюджета) — `tight.len()==1` стабильно,
    /// семантика дропа та же (остаётся самый приоритетный слой — факты).
    #[tokio::test]
    async fn recall_drops_low_priority_layer_when_over_budget() {
        let (dir, db) = open().await;
        let emb = MockEmbedder { dim: DIM };
        let fi = open_index(&dir, "f.usearch");
        let ci = open_index(&dir, "c.usearch");
        let ei = open_index(&dir, "e.usearch");

        // Запрос (для семантического матча всех трёх индексов — MockEmbedder детерминирован по тексту).
        let query = "проект про векторный поиск заметок второго мозга";
        // Факты — КОРОТКИЕ (самый приоритетный слой, должен влезть). Переписка/эпизоды — ДЛИННЫЕ тела
        // (менее приоритетные, должны выпасть): длинный текст → стоимость слоя в СОТНИ токенов, кратно
        // больше любого маркер-дрейфа, поэтому разрыв с бюджетом недвусмыслен (см. док-коммент выше).
        let long_body = format!("{query}. ").repeat(80); // сотни токенов
        seed_fact(&db, &fi, &emb, query).await;
        seed_chat(&db, &ci, &emb, &long_body).await;
        seed_episode(&db, &ei, &emb, &long_body).await;

        let mem = VaultAgentMemory::new(
            db.reader().clone(),
            db.writer().clone(),
            Some(Arc::new(emb)),
            Some(fi),
            Some(ci),
            Some(ei),
            None,
        );
        // Щедрый бюджет → все три слоя (порядок приоритета: переписка, эпизоды, факты — факты ПОСЛЕДНИЕ).
        let full = mem.recall(query, 100_000).await;
        assert_eq!(full.len(), 3, "при щедром бюджете — три слоя");
        let tk = QwenTokenizer::embedded();
        let facts_cost = ContextBudget::message_cost(&tk, full.last().unwrap());
        let episodes_cost = ContextBudget::message_cost(&tk, &full[1]);
        let chat_cost = ContextBudget::message_cost(&tk, &full[0]);
        // GAP кратно больше маркер-дрейфа (несколько токенов), но кратно меньше стоимости менее
        // приоритетных слоёв — граница недвусмысленна (детерминизм, см. док-коммент теста).
        const GAP: usize = 64;
        let budget = facts_cost + GAP;
        // Тест-инвариант разрыва: бюджет ДАЛЕКО ниже даже самого дешёвого менее приоритетного слоя
        // (с большим запасом — не «на границе»). Это и делает результат детерминированным.
        assert!(
            budget + GAP < facts_cost + episodes_cost.min(chat_cost),
            "тест-инвариант: бюджет с двойным запасом ниже двух слоёв \
             (facts={facts_cost}, ep={episodes_cost}, chat={chat_cost})"
        );

        let tight = mem.recall(query, budget).await;
        assert_eq!(
            tight.len(),
            1,
            "в обрезанный бюджет влез ровно один слой (факты); переписка/эпизоды кратно дороже бюджета"
        );
        assert!(
            tight[0].content.contains("сохранённые факты"),
            "остался самый приоритетный слой — факты: {}",
            tight[0].content
        );
    }

    /// AGENT-MEM-1: pack_within_budget — чистая функция упаковки. Дропает слои НАИМЕНЬШЕГО приоритета
    /// (голову) первыми, сохраняя порядок; нулевой бюджет → пусто; щедрый → все. Детерминирован (без
    /// случайного маркера/БД) — точно проверяет семантику дропа.
    #[test]
    fn pack_drops_lowest_priority_first() {
        let tk = QwenTokenizer::embedded();
        // Порядок ВОЗРАСТАНИЯ приоритета: low → mid → high (high ближе к задаче, дропается последним).
        let layers = || {
            vec![
                ChatMessage::user("низкий приоритет — переписка"),
                ChatMessage::user("средний приоритет — эпизоды"),
                ChatMessage::user("высокий приоритет — факты"),
            ]
        };
        let cost = |m: &ChatMessage| ContextBudget::message_cost(&tk, m);
        let ls = layers();
        let (c_low, c_mid, c_high) = (cost(&ls[0]), cost(&ls[1]), cost(&ls[2]));

        // Бюджет на все три → все три, в исходном порядке.
        let all = pack_within_budget(&tk, layers(), c_low + c_mid + c_high);
        assert_eq!(all.len(), 3);
        assert!(all[2].content.contains("факты"));

        // Бюджет ровно на high → остаётся ТОЛЬКО high (low/mid выпали — они менее приоритетны).
        let one = pack_within_budget(&tk, layers(), c_high);
        assert_eq!(one.len(), 1);
        assert!(one[0].content.contains("факты"), "остался high: {one:?}");

        // Бюджет на high+mid → high и mid (low выпал); порядок сохранён (mid перед high).
        let two = pack_within_budget(&tk, layers(), c_high + c_mid);
        assert_eq!(two.len(), 2);
        assert!(two[0].content.contains("эпизоды"));
        assert!(two[1].content.contains("факты"));

        // Нулевой бюджет → пусто.
        assert!(pack_within_budget(&tk, layers(), 0).is_empty());
    }

    /// AGENT-MEM-1 (graceful degrade): None эмбеддер + None индексы → recall пуст (не ошибка).
    #[tokio::test]
    async fn recall_degrades_to_empty_without_ai() {
        let (_dir, db) = open().await;
        let mem = VaultAgentMemory::new(
            db.reader().clone(),
            db.writer().clone(),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(
            mem.recall("любой запрос", 4096).await.is_empty(),
            "без эмбеддера/индексов recall пуст (degrade, не ошибка)"
        );
        // Пустой запрос / нулевой бюджет → тоже пусто (guard).
        let emb = MockEmbedder { dim: DIM };
        let dir = TempDir::new().unwrap();
        let fi = Arc::new(VectorIndex::open(dir.path().join("f.usearch"), DIM).unwrap());
        let mem2 = VaultAgentMemory::new(
            db.reader().clone(),
            db.writer().clone(),
            Some(Arc::new(emb)),
            Some(fi),
            None,
            None,
            None,
        );
        assert!(mem2.recall("  ", 4096).await.is_empty(), "пустой запрос");
        assert!(mem2.recall("q", 0).await.is_empty(), "нулевой бюджет");
    }

    /// AGENT-MEM-1: remember = Add. Первая запись → is_new=true (строка появилась); повтор того же
    /// текста → is_new=false (дубль, не плодим). Пустой текст → false.
    #[tokio::test]
    async fn remember_is_add_only_with_dedup() {
        let (_dir, db) = open().await;
        let mem = VaultAgentMemory::new(
            db.reader().clone(),
            db.writer().clone(),
            None,
            None,
            None,
            None,
            None,
        );
        let text = "пользователь предпочитает Rust для бэкенда";
        assert!(mem.remember(text).await.unwrap(), "первая Add → новая");
        // Строка реально появилась с source='agent'.
        let rows = crate::memory::list(db.reader()).await.unwrap();
        let row = rows.iter().find(|f| f.text == text).expect("факт записан");
        assert_eq!(row.source, SOURCE_AGENT, "source='agent'");
        assert!(!row.pinned, "Add не пинит");

        assert!(
            !mem.remember(text).await.unwrap(),
            "точный дубль → is_new=false (не плодим)"
        );
        // Всё ещё одна строка с этим текстом.
        let count = crate::memory::list(db.reader())
            .await
            .unwrap()
            .iter()
            .filter(|f| f.text == text)
            .count();
        assert_eq!(count, 1, "дубль не создал вторую строку");

        // Пустой текст → не записан, не «новое».
        assert!(!mem.remember("   ").await.unwrap(), "пустой текст не Add");
    }

    /// MockAgentMemory: канонический recall возвращается как есть; remember пишет в журнал и
    /// возвращает заданный is_new (доказывает мокабельность БЕЗ БД/эмбеддера).
    #[tokio::test]
    async fn mock_records_remember_and_returns_canned() {
        let canned = vec![ChatMessage::user("⟦x⟧\nфакт\nканон\n⟦x⟧")];
        let mock = MockAgentMemory::with_canned(canned.clone());
        // ChatMessage не PartialEq — сверяем по роли/контенту.
        let got = mock.recall("q", 10).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].role, canned[0].role);
        assert_eq!(got[0].content, canned[0].content);
        assert!(mock.remember("факт-1").await.unwrap());
        assert!(mock.remember("факт-2").await.unwrap());
        assert_eq!(
            *mock.remembered.lock().unwrap(),
            vec!["факт-1".to_string(), "факт-2".to_string()]
        );
    }
}
