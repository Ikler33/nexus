# Архитектурный план: LLM-Native Knowledge Base
> Obsidian-форк с глубокой интеграцией локальных LLM  
> Версия документа: 1.0 | Статус: Living Document

---

## Оглавление

1. [Обзор и принципы](#1-обзор-и-принципы)
2. [Структура репозитория](#2-структура-репозитория)
3. [Стек технологий](#3-стек-технологий)
4. [Архитектура слоёв](#4-архитектура-слоёв)
   - 4.1 [UI Layer — React + Tauri](#41-ui-layer)
   - 4.2 [Core Layer — Rust (Tauri backend)](#42-core-layer--rust)
   - 4.3 [AI Layer — LLM & RAG Pipeline](#43-ai-layer)
   - 4.4 [Plugin System](#44-plugin-system)
   - 4.5 [Sync Layer — Git-based](#45-sync-layer)
5. [Схема данных](#5-схема-данных)
6. [AI-пайплайн подробно](#6-ai-пайплайн-подробно)
7. [Плагинная система подробно](#7-плагинная система-подробно)
8. [Git-sync протокол](#8-git-sync-протокол)
9. [i18n архитектура](#9-i18n-архитектура)
10. [Производительность и масштабирование до 50k+ файлов](#10-производительность)
11. [Безопасность](#11-безопасность)
12. [Фазы разработки](#12-фазы-разработки)
13. [Риски и решения](#13-риски-и-решения)

---

## 1. Обзор и принципы

### Название проекта
**Nexus** — локально-первый knowledge base с LLM-нативной архитектурой.

### Ключевые принципы (non-negotiable)

| Принцип | Следствие |
|---|---|
| **Local-first** | Всё работает без интернета. Облако — опция |
| **Plain files** | Vault = папка с `.md` файлами. Совместимость с Obsidian |
| **Plugin-first** | Каждая необязательная фича — плагин. Ядро минимально |
| **AI as layer** | LLM — отдельный слой, не вплетён в ядро |
| **Correctness over speed** | Архитектура правильная сразу. Оптимизация потом |
| **Privacy by default** | Данные не покидают устройство без явного разрешения |
| **Open protocol** | Формат синхронизации документирован и открыт |

### Что делает этот форк принципиально другим

```
Obsidian:  файлы → парсер → граф → UI
Nexus:     файлы → парсер → граф + vector index → UI + LLM context
                                        ↑
                              живой слой, всегда актуальный
```

LLM не добавлен поверх — он встроен в индексный слой. Каждый документ при сохранении автоматически переиндексируется, обновляет граф обратных ссылок и вектор-индекс. Чат с LLM, предложения связей, автосаммари — всё работает на одном и том же актуальном индексе.

---

## 2. Структура репозитория

```
nexus/
├── apps/
│   ├── desktop/                    # Tauri приложение
│   │   ├── src/                    # React + TypeScript frontend
│   │   │   ├── components/
│   │   │   │   ├── editor/         # CodeMirror 6 редактор
│   │   │   │   ├── graph/          # D3 / sigma.js граф
│   │   │   │   ├── sidebar/        # Файловое дерево, поиск
│   │   │   │   ├── ai-panel/       # Чат, подсказки, связи
│   │   │   │   └── plugin-ui/      # Точки рендера плагинов
│   │   │   ├── hooks/              # React hooks
│   │   │   ├── stores/             # Zustand stores
│   │   │   ├── i18n/               # Локализация RU/EN
│   │   │   │   ├── ru.json
│   │   │   │   └── en.json
│   │   │   ├── plugin-api/         # TypeScript API для плагинов
│   │   │   │   ├── types.ts
│   │   │   │   └── hooks.ts
│   │   │   └── main.tsx
│   │   └── src-tauri/              # Rust backend
│   │       ├── src/
│   │       │   ├── main.rs
│   │       │   ├── commands/       # Tauri команды (IPC)
│   │       │   │   ├── vault.rs
│   │       │   │   ├── search.rs
│   │       │   │   ├── ai.rs
│   │       │   │   ├── git.rs
│   │       │   │   └── plugin.rs
│   │       │   ├── vault/          # Файловая система vault
│   │       │   │   ├── watcher.rs  # notify-rs watcher
│   │       │   │   ├── parser.rs   # MD парсер + link extractor
│   │       │   │   └── indexer.rs  # Инкрементальный индексатор
│   │       │   ├── graph/          # Граф обратных ссылок
│   │       │   │   ├── store.rs    # In-memory граф
│   │       │   │   └── queries.rs
│   │       │   ├── ai/             # AI слой
│   │       │   │   ├── client.rs   # HTTP клиент к llama.cpp
│   │       │   │   ├── embedder.rs # Запросы к embedding endpoint
│   │       │   │   ├── rag.rs      # RAG pipeline
│   │       │   │   └── suggest.rs  # Предложения связей
│   │       │   ├── db/             # SQLite слой
│   │       │   │   ├── migrations/ # SQL миграции
│   │       │   │   ├── schema.rs
│   │       │   │   └── queries.rs
│   │       │   ├── git/            # Git sync
│   │       │   │   ├── ops.rs      # git2-rs операции
│   │       │   │   └── conflict.rs # Разрешение конфликтов
│   │       │   └── plugin/         # Plugin runtime
│   │       │       ├── loader.rs   # Загрузка плагинов
│   │       │       ├── sandbox.rs  # WASM sandbox
│   │       │       └── registry.rs
│   │       ├── Cargo.toml
│   │       └── tauri.conf.json
│   └── mobile/                     # Будущее: React Native / Tauri mobile
│       └── .gitkeep
│
├── packages/
│   ├── nexus-plugin-sdk/           # npm пакет: SDK для плагинов
│   │   ├── src/
│   │   │   ├── index.ts
│   │   │   ├── types.ts            # Все публичные типы
│   │   │   └── decorators.ts
│   │   └── package.json
│   ├── nexus-ui-kit/               # Общие UI компоненты
│   └── nexus-md-parser/            # Shared MD parser (для будущего mobile)
│
├── plugins/                        # First-party плагины
│   ├── nexus-graph-view/           # Граф (встроен, но как плагин)
│   ├── nexus-daily-notes/
│   ├── nexus-templates/
│   ├── nexus-git-sync/             # Git sync как плагин
│   └── nexus-ai-suggest/           # AI подсказки как плагин
│
├── docs/
│   ├── architecture/               # Этот документ и его обновления
│   ├── plugin-api/                 # Документация API плагинов
│   └── sync-protocol.md            # Спецификация git-sync протокола
│
├── scripts/
│   ├── dev.sh                      # Запуск dev окружения
│   └── build.sh
│
├── pnpm-workspace.yaml             # Monorepo
└── Cargo.toml                      # Workspace для Rust крейтов
```

---

## 3. Стек технологий

### Frontend

| Компонент | Технология | Обоснование |
|---|---|---|
| Framework | **React 19 + TypeScript** | Самая большая экосистема, хорошая поддержка CodeMirror |
| Build | **Vite 6** | Быстрый HMR, нативный ESM |
| State | **Zustand** | Минималистичный, без boilerplate, легко тестировать |
| Editor | **CodeMirror 6** | Тот же что в Obsidian, расширяемый, производительный |
| Graph | **sigma.js 3 + graphology** | WebGL, держит 100k+ нод; D3 для небольших графов (<5k) |
| Styling | **CSS Modules + CSS Variables** | Без runtime overhead, theme через переменные |
| i18n | **i18next + react-i18next** | Де-факто стандарт, lazy loading переводов |
| Icons | **Lucide React** | Легковесные, последовательные |
| Testing | **Vitest + React Testing Library** | |

### Backend (Rust / Tauri)

| Компонент | Технология | Обоснование |
|---|---|---|
| Desktop shell | **Tauri 2** | <5MB бандл, нативный webview, Rust IPC |
| File watching | **notify-rs** | Кроссплатформенный, debounce встроен |
| MD parsing | **pulldown-cmark** | Быстрый, CommonSpec, кастомные события |
| Database | **SQLite via sqlx** | Embedded, async, строгая типизация |
| Vector store | **sqlite-vec** | Расширение SQLite, один файл БД, ANN поиск |
| Git | **git2-rs** | libgit2 биндинги, полный контроль |
| HTTP client | **reqwest** | Async, для запросов к llama.cpp сервер |
| Plugin runtime | **Wasmtime** | WASM sandbox для плагинов |
| Serialization | **serde + serde_json** | |
| Async runtime | **Tokio** | |
| Logging | **tracing** | Структурированные логи |

### AI инфраструктура

| Компонент | Технология |
|---|---|
| LLM inference | **llama.cpp HTTP server** (уже запущен, Qwen3 27B) |
| Embeddings | **llama.cpp embedding endpoint** (тот же сервер) |
| RAG framework | Нативная реализация на Rust (без Python зависимостей) |
| Vector search | **sqlite-vec** (HNSW индекс) |
| Chunking | Кастомный chunker на Rust с учётом markdown структуры |
| Облачный fallback | OpenAI API / Anthropic API (через тот же интерфейс) |

---

## 4. Архитектура слоёв

### 4.1 UI Layer

```
┌─────────────────────────────────────────────────────────────┐
│                        React App                            │
│  ┌──────────┐  ┌──────────────┐  ┌────────────────────┐   │
│  │ Sidebar  │  │    Editor    │  │     AI Panel       │   │
│  │          │  │              │  │                    │   │
│  │ FileTree │  │ CodeMirror 6 │  │  Chat Interface    │   │
│  │ Search   │  │              │  │  Link Suggestions  │   │
│  │ Tags     │  │ Live Preview │  │  Auto-Summary      │   │
│  └──────────┘  └──────────────┘  └────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                   Graph View                        │   │
│  │              sigma.js / WebGL                       │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              Plugin UI Slots                        │   │
│  │   [left-sidebar] [right-panel] [status-bar] [cmd]  │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
│           Zustand Stores ←→ Tauri IPC (invoke/event)       │
└─────────────────────────────────────────────────────────────┘
```

**Zustand stores:**

```typescript
// stores/vault.ts
interface VaultStore {
  currentFile: NoteFile | null;
  recentFiles: NoteFile[];
  openFile: (path: string) => Promise<void>;
  saveFile: (path: string, content: string) => Promise<void>;
}

// stores/graph.ts
interface GraphStore {
  nodes: GraphNode[];
  edges: GraphEdge[];
  filters: GraphFilter;
  focusNode: (id: string) => void;
}

// stores/ai.ts
interface AIStore {
  chatMessages: ChatMessage[];
  isIndexing: boolean;
  indexProgress: IndexProgress | null;
  sendMessage: (msg: string, context: RAGContext) => AsyncIterator<string>;
  suggestions: LinkSuggestion[];
}

// stores/plugin.ts
interface PluginStore {
  installed: PluginManifest[];
  enabled: Set<string>;
  togglePlugin: (id: string) => Promise<void>;
}
```

**Tauri IPC — все команды типизированы:**

```typescript
// lib/tauri-api.ts — единственное место где вызываем invoke
export const tauriApi = {
  vault: {
    readFile: (path: string) => invoke<string>('read_file', { path }),
    writeFile: (path: string, content: string) => invoke<void>('write_file', { path, content }),
    listFiles: (vaultPath: string) => invoke<FileEntry[]>('list_files', { vaultPath }),
    searchFullText: (query: string) => invoke<SearchResult[]>('search_full_text', { query }),
  },
  graph: {
    getGraph: () => invoke<GraphData>('get_graph'),
    getBacklinks: (path: string) => invoke<BacklinkEntry[]>('get_backlinks', { path }),
  },
  ai: {
    searchSemantic: (query: string, topK: number) => invoke<SemanticResult[]>('semantic_search', { query, topK }),
    getSuggestions: (path: string) => invoke<LinkSuggestion[]>('get_link_suggestions', { path }),
    // Чат через streaming event, не invoke:
    startChat: (sessionId: string, messages: ChatMessage[], context: string[]) =>
      invoke<void>('start_chat_stream', { sessionId, messages, context }),
  },
  git: {
    getStatus: () => invoke<GitStatus>('git_status'),
    commit: (message: string) => invoke<void>('git_commit', { message }),
    pull: () => invoke<PullResult>('git_pull'),
    push: () => invoke<void>('git_push'),
  },
};
```

**Streaming LLM ответов через Tauri events:**

```typescript
// hooks/useChat.ts
export function useChat() {
  const [response, setResponse] = useState('');

  useEffect(() => {
    const unlisten = listen<StreamChunk>('llm-stream', (event) => {
      if (event.payload.done) return;
      setResponse(prev => prev + event.payload.text);
    });
    return () => { unlisten.then(f => f()); };
  }, []);

  const send = async (messages: ChatMessage[], context: string[]) => {
    setResponse('');
    const sessionId = crypto.randomUUID();
    await tauriApi.ai.startChat(sessionId, messages, context);
  };

  return { response, send };
}
```

---

### 4.2 Core Layer — Rust

```
┌─────────────────────────────────────────────────────────┐
│                   Tauri Commands                        │
│         (typed IPC bridge, async Tokio tasks)           │
└────────────────────┬────────────────────────────────────┘
                     │
     ┌───────────────┼───────────────┐
     │               │               │
┌────▼─────┐   ┌─────▼────┐   ┌─────▼─────┐
│  Vault   │   │  Graph   │   │  AI Core  │
│  Manager │   │  Store   │   │  (RAG)    │
└────┬─────┘   └─────┬────┘   └─────┬─────┘
     │               │               │
     └───────────────▼───────────────┘
                      │
              ┌───────▼────────┐
              │  SQLite + vec  │
              │  (единый файл) │
              └────────────────┘
```

**Vault Manager** — сердце системы:

```rust
// vault/watcher.rs
pub struct VaultWatcher {
    watcher: RecommendedWatcher,
    tx: mpsc::Sender<VaultEvent>,
}

pub enum VaultEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

impl VaultWatcher {
    pub fn new(vault_path: &Path, tx: mpsc::Sender<VaultEvent>) -> Result<Self> {
        let mut watcher = notify::recommended_watcher(move |res| {
            // debounce 300ms встроен через notify::Config
        })?;
        watcher.watch(vault_path, RecursiveMode::Recursive)?;
        Ok(Self { watcher, tx })
    }
}

// vault/indexer.rs — инкрементальный индексатор
pub struct VaultIndexer {
    db: Arc<DbPool>,
    ai_client: Arc<AIClient>,
}

impl VaultIndexer {
    // Вызывается при каждом VaultEvent::Modified
    pub async fn reindex_file(&self, path: &Path) -> Result<()> {
        let content = fs::read_to_string(path).await?;
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();

        // Пропускаем если контент не изменился
        let stored_hash = self.db.get_file_hash(path).await?;
        if stored_hash.as_deref() == Some(&hash) {
            return Ok(());
        }

        // Параллельно: парсинг + embedding
        let (parsed, embedding) = tokio::join!(
            self.parse_file(&content),
            self.ai_client.embed(&content),
        );

        let parsed = parsed?;
        let embedding = embedding?;

        // Транзакция: обновляем всё атомарно
        self.db.transaction(|tx| async move {
            tx.upsert_file(path, &hash, &parsed.frontmatter).await?;
            tx.update_links(path, &parsed.outgoing_links).await?;
            tx.update_tags(path, &parsed.tags).await?;
            tx.upsert_chunks(path, &parsed.chunks, &embedding).await?;
            Ok(())
        }).await
    }
}
```

**Graph Store** — граф в памяти для быстрых запросов:

```rust
// graph/store.rs
use petgraph::graphmap::DiGraphMap;

pub struct GraphStore {
    // Направленный граф: A -> B означает "A ссылается на B"
    graph: RwLock<DiGraphMap<FileId, LinkMetadata>>,
    path_to_id: DashMap<PathBuf, FileId>,
    id_to_path: DashMap<FileId, PathBuf>,
}

impl GraphStore {
    // O(1) получение обратных ссылок
    pub fn get_backlinks(&self, file_id: FileId) -> Vec<FileId> {
        let g = self.graph.read();
        g.neighbors_directed(file_id, Direction::Incoming).collect()
    }

    // При старте — загружаем из SQLite в память (~50ms для 50k файлов)
    pub async fn hydrate_from_db(&self, db: &DbPool) -> Result<()> {
        let links = db.get_all_links().await?;
        let mut g = self.graph.write();
        for link in links {
            g.add_edge(link.source_id, link.target_id, link.metadata);
        }
        Ok(())
    }
}
```

---

### 4.3 AI Layer

```
┌────────────────────────────────────────────────────────────┐
│                      AI Client                             │
│                                                            │
│  ┌─────────────────────────────────────────────────────┐  │
│  │              LLM Provider Abstraction               │  │
│  │                                                     │  │
│  │  trait LLMProvider {                                │  │
│  │    async fn complete(&self, ...) -> Stream<String>  │  │
│  │    async fn embed(&self, text: &str) -> Vec<f32>    │  │
│  │  }                                                  │  │
│  │                                                     │  │
│  │  ┌──────────────┐  ┌───────────┐  ┌─────────────┐  │  │
│  │  │ LlamaCpp     │  │  OpenAI   │  │  Anthropic  │  │  │
│  │  │ Provider     │  │ Provider  │  │  Provider   │  │  │
│  │  │ (default)    │  │ (optional)│  │  (optional) │  │  │
│  │  └──────────────┘  └───────────┘  └─────────────┘  │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                            │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────────┐ │
│  │  Embedder   │  │ RAG Pipeline │  │  Link Suggester   │ │
│  └─────────────┘  └──────────────┘  └───────────────────┘ │
└────────────────────────────────────────────────────────────┘
```

**Trait для провайдеров:**

```rust
// ai/provider.rs
#[async_trait]
pub trait LLMProvider: Send + Sync {
    async fn complete(
        &self,
        messages: &[Message],
        options: &CompletionOptions,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>>;

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    fn max_context_tokens(&self) -> usize;
    fn provider_name(&self) -> &str;
}

// ai/client.rs — фасад, выбирает провайдера
pub struct AIClient {
    primary: Arc<dyn LLMProvider>,    // llama.cpp по умолчанию
    fallback: Option<Arc<dyn LLMProvider>>, // OpenAI/Anthropic если настроен
}

impl AIClient {
    pub async fn complete(&self, ...) -> Result<...> {
        match self.primary.complete(...).await {
            Ok(stream) => Ok(stream),
            Err(e) if self.fallback.is_some() => {
                warn!("Primary LLM failed: {e}, falling back");
                self.fallback.as_ref().unwrap().complete(...).await
            }
            Err(e) => Err(e),
        }
    }
}
```

---

### 4.4 Plugin System

Плагинная система — это отдельная подсистема. Подробно описана в [разделе 7](#7-плагинная-система-подробно).

**Три типа плагинов:**

```
1. WASM-плагины    — изолированы в sandbox, безопасны, для логики
2. JS-плагины      — запускаются в изолированном iframe, для UI
3. Native-плагины  — Rust крейты (только первой стороны, подписанные)
```

---

### 4.5 Sync Layer

Git-based синхронизация описана в [разделе 8](#8-git-sync-протокол).

---

## 5. Схема данных

### SQLite — основная БД (`nexus.db` внутри vault/.nexus/)

```sql
-- Файлы vault
CREATE TABLE files (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,  -- относительный путь от корня vault
    hash        TEXT NOT NULL,          -- blake3 хэш контента
    title       TEXT,                   -- из frontmatter или первого H1
    created_at  INTEGER NOT NULL,       -- unix timestamp из frontmatter или fs
    updated_at  INTEGER NOT NULL,
    indexed_at  INTEGER NOT NULL,       -- когда последний раз индексировали
    size_bytes  INTEGER NOT NULL,
    word_count  INTEGER NOT NULL DEFAULT 0,
    frontmatter TEXT,                   -- JSON blob всего frontmatter
    is_deleted  INTEGER NOT NULL DEFAULT 0  -- soft delete
);

-- Исходящие ссылки
CREATE TABLE links (
    id          INTEGER PRIMARY KEY,
    source_id   INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    target_id   INTEGER REFERENCES files(id) ON DELETE SET NULL,
    target_raw  TEXT NOT NULL,          -- оригинальный текст [[ссылки]]
    link_type   TEXT NOT NULL,          -- 'wikilink' | 'markdown' | 'embed'
    context     TEXT,                   -- ~100 символов вокруг ссылки
    line_number INTEGER
);

-- Теги
CREATE TABLE tags (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL UNIQUE        -- нормализованный тег (lowercase)
);

CREATE TABLE file_tags (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (file_id, tag_id)
);

-- Алиасы файлов (из frontmatter aliases: [...])
CREATE TABLE aliases (
    id      INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    alias   TEXT NOT NULL,
    UNIQUE(alias)
);

-- Чанки для RAG
CREATE TABLE chunks (
    id          INTEGER PRIMARY KEY,
    file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    content     TEXT NOT NULL,
    char_start  INTEGER NOT NULL,
    char_end    INTEGER NOT NULL,
    heading_path TEXT,                  -- H1 > H2 > H3 путь к чанку
    token_count INTEGER NOT NULL
);

-- Full-text search (FTS5)
CREATE VIRTUAL TABLE fts_files USING fts5(
    title,
    content,
    content=files,         -- linked mode: не дублируем данные
    content_rowid=id
);

-- sqlite-vec: векторный индекс чанков
-- (создаётся через sqlite-vec расширение)
CREATE VIRTUAL TABLE vec_chunks USING vec0(
    chunk_id INTEGER PRIMARY KEY,
    embedding FLOAT[1024]  -- размер зависит от модели
);

-- История чатов с AI
CREATE TABLE chat_sessions (
    id          TEXT PRIMARY KEY,       -- UUID
    title       TEXT,
    created_at  INTEGER NOT NULL,
    context_paths TEXT                  -- JSON array путей файлов в контексте
);

CREATE TABLE chat_messages (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,          -- 'user' | 'assistant' | 'system'
    content     TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    metadata    TEXT                    -- JSON: sources, tokens, etc.
);

-- Кэш предложений связей (чтобы не пересчитывать при каждом открытии)
CREATE TABLE link_suggestions (
    file_id         INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    suggested_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    score           REAL NOT NULL,
    reason          TEXT,               -- краткое объяснение
    generated_at    INTEGER NOT NULL,
    dismissed       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (file_id, suggested_id)
);

-- Настройки приложения
CREATE TABLE settings (
    key     TEXT PRIMARY KEY,
    value   TEXT NOT NULL               -- JSON value
);

-- Индексы
CREATE INDEX idx_links_source ON links(source_id);
CREATE INDEX idx_links_target ON links(target_id);
CREATE INDEX idx_file_tags_file ON file_tags(file_id);
CREATE INDEX idx_chunks_file ON chunks(file_id);
CREATE INDEX idx_files_updated ON files(updated_at);
```

### Файловая структура vault

```
my-vault/
├── .nexus/                     # Служебная директория (в .gitignore частично)
│   ├── nexus.db                # SQLite база (в .gitignore)
│   ├── nexus.db-vec            # sqlite-vec данные (в .gitignore)
│   ├── config.json             # Настройки vault (в git)
│   ├── plugins/                # Установленные плагины (в git)
│   │   └── plugin-name/
│   └── themes/                 # Темы оформления
├── Notes/
│   ├── My Note.md
│   └── Another Note.md
├── Attachments/
│   └── image.png
└── .gitignore                  # Игнорирует nexus.db, но включает config.json
```

### `.nexus/config.json` — конфиг vault (синхронизируется через git)

```json
{
  "version": "1.0",
  "vault_name": "My Knowledge Base",
  "ai": {
    "primary_provider": "llamacpp",
    "llamacpp": {
      "base_url": "http://localhost:8080",
      "model": "qwen3-27b",
      "embedding_model": "nomic-embed-text",
      "context_window": 32768
    },
    "fallback_provider": null,
    "auto_suggest_links": true,
    "suggest_on_save": true,
    "max_rag_chunks": 8
  },
  "sync": {
    "enabled": false,
    "remote": null,
    "auto_commit": true,
    "auto_commit_interval_sec": 300,
    "auto_pull": true
  },
  "plugins": {
    "enabled": ["nexus-graph-view", "nexus-daily-notes"]
  },
  "editor": {
    "vim_mode": false,
    "font_size": 16,
    "line_width": 720
  },
  "locale": "ru"
}
```

---

## 6. AI-пайплайн подробно

### 6.1 Индексирование

```
Vault файл изменён
        │
        ▼
┌───────────────┐
│  File Watcher │  notify-rs, debounce 500ms
└───────┬───────┘
        │
        ▼
┌───────────────┐
│ Hash Check    │  blake3, пропускаем если не изменился
└───────┬───────┘
        │ изменился
        ▼
┌───────────────────────────────────────────────┐
│              Parallel Processing              │
│                                               │
│  ┌─────────────┐        ┌──────────────────┐  │
│  │ MD Parser   │        │   Chunker        │  │
│  │             │        │                  │  │
│  │ - frontmatter│       │ Стратегия:       │  │
│  │ - [[links]] │        │ 1. По заголовкам │  │
│  │ - #tags     │        │ 2. Sliding window│  │
│  │ - headings  │        │    512 tok,      │  │
│  └─────────────┘        │    overlap 64    │  │
│                         └────────┬─────────┘  │
└────────────────────────────┬─────┘────────────┘
                             │
                             ▼
                  ┌──────────────────┐
                  │   Embedder       │
                  │                  │
                  │ POST /embedding  │
                  │ к llama.cpp      │
                  │ batch size: 32   │
                  └────────┬─────────┘
                           │
                           ▼
                  ┌──────────────────┐
                  │  SQLite Commit   │
                  │  (транзакция)    │
                  │                  │
                  │ - files          │
                  │ - links          │
                  │ - tags           │
                  │ - chunks         │
                  │ - vec_chunks     │
                  │ - fts update     │
                  └──────────────────┘
```

**Умный чанкер — ключевой компонент:**

```rust
// ai/chunker.rs
pub struct MarkdownChunker {
    max_tokens: usize,      // 512
    overlap_tokens: usize,  // 64
}

impl MarkdownChunker {
    pub fn chunk(&self, content: &str, parsed: &ParsedDocument) -> Vec<Chunk> {
        // Стратегия 1: сначала делим по заголовкам
        let sections = self.split_by_headings(parsed);

        let mut chunks = Vec::new();
        for section in sections {
            if section.token_count <= self.max_tokens {
                // Секция влезает целиком — хорошо
                chunks.push(Chunk {
                    content: section.content,
                    heading_path: section.heading_path.clone(),
                    ..
                });
            } else {
                // Секция большая — sliding window с учётом предложений
                chunks.extend(self.sliding_window(&section));
            }
        }

        // Добавляем overlap: каждый чанк получает 64 токена из предыдущего
        self.add_overlap(&mut chunks);

        chunks
    }
}
```

### 6.2 RAG Pipeline (при запросе)

```
Пользователь задаёт вопрос
        │
        ▼
┌───────────────────────────────────────────────────────┐
│                  Context Builder                      │
│                                                       │
│  1. Embed вопрос → вектор запроса                    │
│                                                       │
│  2. Hybrid Search:                                    │
│     ┌─────────────────┐   ┌────────────────────┐     │
│     │ Vector Search   │   │  FTS5 Full-text    │     │
│     │ sqlite-vec ANN  │   │  BM25 ranking      │     │
│     │ top-20 chunks   │   │  top-20 chunks     │     │
│     └────────┬────────┘   └─────────┬──────────┘     │
│              └──────────┬───────────┘                 │
│                         │                             │
│  3. RRF Fusion          ▼                             │
│     Reciprocal Rank Fusion → топ 10 чанков            │
│                                                       │
│  4. Graph Boost:                                       │
│     Если текущий файл открыт → +0.2 для чанков       │
│     из файлов, на которые он ссылается                │
│                                                       │
│  5. Re-rank (опционально):                            │
│     cross-encoder через llama.cpp если < 200ms        │
│                                                       │
└──────────────────────┬────────────────────────────────┘
                       │  топ-8 чанков с метаданными
                       ▼
┌──────────────────────────────────────────────────────┐
│                  Prompt Builder                      │
│                                                      │
│  System: "Ты помощник по knowledge base. Отвечай    │
│  на основе предоставленных заметок. Если информации │
│  нет — скажи об этом."                              │
│                                                      │
│  Context:                                            │
│  <note path="Projects/Alpha.md" score="0.92">       │
│  ...контент чанка...                                 │
│  </note>                                             │
│  ...ещё 7 чанков...                                  │
│                                                      │
│  User: {вопрос пользователя}                         │
└──────────────────────┬───────────────────────────────┘
                       │
                       ▼
              llama.cpp /v1/chat/completions
              (streaming, SSE)
                       │
                       ▼
              Tauri event → React UI
              (токен за токеном)
```

### 6.3 Предложения связей

Два режима, оба работают асинхронно и не блокируют редактор:

**Режим 1 — Embedding similarity (быстрый, ~100ms):**
```rust
// ai/suggest.rs
pub async fn suggest_by_similarity(
    &self,
    file_id: FileId,
    db: &DbPool,
) -> Result<Vec<LinkSuggestion>> {
    // Берём embedding самого документа (среднее по чанкам)
    let file_embedding = db.get_file_embedding(file_id).await?;

    // ANN поиск по sqlite-vec, исключая уже существующие ссылки
    let candidates = db.vec_search(
        &file_embedding,
        top_k: 20,
        exclude: db.get_linked_files(file_id).await?,
    ).await?;

    // Фильтруем по порогу сходства (>= 0.75)
    Ok(candidates.into_iter()
        .filter(|c| c.score >= 0.75)
        .take(5)
        .collect())
}
```

**Режим 2 — LLM reasoning (умный, ~2-5s):**
```rust
pub async fn suggest_by_llm(
    &self,
    file_path: &Path,
    content: &str,
    candidates: &[FileMetadata],  // результат режима 1
    llm: &AIClient,
) -> Result<Vec<LinkSuggestion>> {
    let prompt = format!(
        "Текущая заметка:\n{content}\n\n\
         Кандидаты на связь:\n{candidates_list}\n\n\
         Для каждого кандидата укажи: стоит ли добавить ссылку (да/нет) \
         и краткую причину (1 предложение). Формат: JSON array.",
    );
    // LLM возвращает структурированный JSON с оценкой
    let result = llm.complete_json(&prompt).await?;
    parse_suggestions(result)
}
```

### 6.4 Контекстное окно при большом vault

При 50k+ файлов нельзя отправить всё в LLM. Стратегия:

```
Лимит контекста: 32768 токенов (Qwen3 27B)
Резерв для ответа: 4096 токенов
Доступно для контекста: ~28000 токенов

Распределение:
├── System prompt:          ~500 токенов
├── Текущий файл:        до 4000 токенов  (если открыт)
├── RAG чанки:           до 8000 токенов  (8 чанков × 512 + overlap)
├── Backlink контекст:   до 2000 токенов  (краткие выжимки из связанных файлов)
├── История чата:        до 6000 токенов  (последние N сообщений)
└── Вопрос пользователя:   ~500 токенов
                         ────────────────
Итого:                  ~21000 токенов   ✓ влезает с запасом
```

---

## 7. Плагинная система подробно

### 7.1 Архитектура

```
┌──────────────────────────────────────────────────────────────┐
│                    Plugin Registry                           │
│                                                              │
│  plugins/
│  ├── installed.json          # список установленных          │
│  └── {plugin-id}/
│      ├── manifest.json       # метаданные плагина            │
│      ├── main.wasm           # WASM модуль (логика)          │
│      └── ui/                 # опциональные UI ресурсы       │
│          ├── index.html      # рендерится в iframe           │
│          └── styles.css                                      │
└──────────────────────────────────────────────────────────────┘
```

### 7.2 Manifest плагина

```json
{
  "id": "nexus-ai-suggest",
  "name": "AI Link Suggestions",
  "version": "1.2.0",
  "api_version": "^1.0",
  "author": "Community",
  "description": "Suggests related notes using semantic similarity",
  "entry": "main.wasm",
  "ui": "ui/index.html",
  "permissions": [
    "vault:read",
    "vault:write",
    "ai:embed",
    "ai:complete",
    "ui:sidebar-right",
    "ui:status-bar"
  ],
  "settings_schema": {
    "threshold": {
      "type": "number",
      "default": 0.75,
      "min": 0.5,
      "max": 1.0,
      "label": "Similarity threshold"
    }
  }
}
```

### 7.3 Plugin API (TypeScript SDK)

```typescript
// packages/nexus-plugin-sdk/src/index.ts

// Плагин объявляет что он хочет делать
export interface NexusPlugin {
  id: string;
  onLoad(api: PluginAPI): Promise<void>;
  onUnload(): Promise<void>;
}

export interface PluginAPI {
  // Vault операции (требуют vault:read / vault:write)
  vault: {
    readFile(path: string): Promise<string>;
    writeFile(path: string, content: string): Promise<void>;
    listFiles(pattern?: string): Promise<string[]>;
    onFileChanged(callback: (path: string) => void): Unsubscribe;
  };

  // AI операции (требуют ai:embed / ai:complete)
  ai: {
    embed(texts: string[]): Promise<number[][]>;
    complete(messages: Message[], opts?: CompletionOpts): AsyncIterable<string>;
    searchSemantic(query: string, topK: number): Promise<SemanticResult[]>;
  };

  // UI расширение (требуют ui:*)
  ui: {
    registerSidebarPanel(config: PanelConfig): void;
    registerStatusBarItem(config: StatusBarConfig): void;
    registerCommand(config: CommandConfig): void;
    registerContextMenu(config: ContextMenuConfig): void;
    registerEditorExtension(ext: EditorExtension): void; // CodeMirror extension
  };

  // Settings
  settings: {
    get<T>(key: string): T | undefined;
    set<T>(key: string, value: T): Promise<void>;
    onChange<T>(key: string, callback: (value: T) => void): Unsubscribe;
  };
}
```

### 7.4 WASM Sandbox (Rust сторона)

```rust
// plugin/sandbox.rs
use wasmtime::*;

pub struct PluginSandbox {
    engine: Engine,
    instances: HashMap<PluginId, PluginInstance>,
}

pub struct PluginInstance {
    store: Store<PluginState>,
    instance: Instance,
    permissions: PermissionSet,
}

impl PluginSandbox {
    pub async fn call_plugin(
        &mut self,
        plugin_id: &PluginId,
        method: &str,
        args: &[Value],
    ) -> Result<Value> {
        let instance = self.instances.get_mut(plugin_id)
            .ok_or(PluginError::NotLoaded)?;

        // Проверка permissions перед вызовом
        self.check_permission(plugin_id, method)?;

        // Вызов WASM функции с timeout
        tokio::time::timeout(
            Duration::from_secs(30),
            instance.call(method, args),
        ).await?
    }
}
```

### 7.5 UI-плагины (iframe изоляция)

```typescript
// UI плагины рендерятся в изолированный iframe
// Общение через postMessage (structured clone)

// Внутри iframe плагина:
const nexus = new NexusPluginUI({
  pluginId: 'nexus-ai-suggest',
  permissions: ['vault:read', 'ui:sidebar-right'],
});

// API автоматически проксируется через postMessage
await nexus.vault.readFile('Notes/example.md');
```

### 7.6 Точки расширения UI

```
┌─────────────────────────────────────────────────────────┐
│  [left-sidebar-top]    [left-sidebar-bottom]            │
│                                                         │
│  [toolbar-left]  [editor-content]  [toolbar-right]      │
│                                                         │
│                  [editor-bottom]                        │
│                                                         │
│  [right-panel-top]                                      │
│  [right-panel-content]     ← основная панель плагина    │
│  [right-panel-bottom]                                   │
│                                                         │
│  [status-bar-left] ............ [status-bar-right]      │
└─────────────────────────────────────────────────────────┘

Дополнительно:
- Command Palette (Cmd+P) — плагины добавляют команды
- Context Menu — правая кнопка в редакторе и файловом дереве
- CodeMirror Extensions — кастомные inline виджеты в редакторе
- Graph overlays — доп. визуализации поверх графа
```

---

## 8. Git-sync протокол

### 8.1 Структура .gitignore

```gitignore
# Nexus: игнорируем индексы (пересоздаются локально)
.nexus/nexus.db
.nexus/nexus.db-vec
.nexus/*.db-*
.nexus/cache/

# Nexus: синхронизируем конфиг и плагины
!.nexus/config.json
!.nexus/plugins/
!.nexus/themes/
```

### 8.2 Auto-commit логика

```rust
// git/ops.rs
pub struct GitSync {
    repo: Repository,
    config: GitConfig,
}

impl GitSync {
    // Вызывается каждые N секунд (настраивается, default 300s)
    // или при явном сохранении файла
    pub async fn auto_commit(&self) -> Result<Option<CommitId>> {
        let status = self.repo.statuses(None)?;

        // Нечего коммитить
        if status.is_empty() { return Ok(None); }

        let changed_files: Vec<String> = status.iter()
            .map(|e| e.path().unwrap_or("unknown").to_string())
            .collect();

        // Генерируем умное сообщение коммита
        let message = self.generate_commit_message(&changed_files);
        // Например: "Update: Projects/Alpha.md, Notes/Ideas.md (+2 more)"

        let mut index = self.repo.index()?;
        index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
        index.write()?;

        let tree_id = index.write_tree()?;
        let tree = self.repo.find_tree(tree_id)?;
        let parent = self.repo.head()?.peel_to_commit()?;

        let sig = self.repo.signature()?;
        let commit_id = self.repo.commit(
            Some("HEAD"),
            &sig, &sig,
            &message,
            &tree,
            &[&parent],
        )?;

        Ok(Some(commit_id))
    }
}
```

### 8.3 Конфликты

Конфликты в markdown файлах неизбежны при совместном использовании. Стратегия:

```
Тип конфликта          Стратегия
─────────────────────────────────────────────────────────
.md файлы              Three-way merge (libgit2)
                       При неудаче: создать Note.md.conflict
                       с обеими версиями, уведомить пользователя

config.json            Ours (локальная конфигурация приоритетнее)

Переименования         Обнаруживаем через similarity (git2),
                       обновляем ссылки автоматически

Бинарные файлы         Ours по умолчанию
(вложения)
```

### 8.4 Будущий мобильный клиент

```
[Desktop]  ←─git pull/push─→  [Remote repo]  ←─git pull/push─→  [Mobile]
    │                                                                  │
    │  .md файлы + config.json синхронизируются                       │
    │                                                                  │
    │  nexus.db НЕ синхронизируется →                                 │
    │  каждое устройство строит свой индекс локально                  │
```

---

## 9. i18n Архитектура

### Структура переводов

```typescript
// i18n/en.json
{
  "app": {
    "title": "Nexus",
    "loading": "Loading vault..."
  },
  "editor": {
    "placeholder": "Start writing...",
    "link_suggestions": "Related notes",
    "no_suggestions": "No suggestions found"
  },
  "ai": {
    "chat_placeholder": "Ask anything about your notes...",
    "indexing": "Indexing {{count}} files...",
    "indexed": "Vault indexed ({{count}} notes)",
    "error_no_llm": "LLM server not available. Check settings."
  },
  "graph": {
    "title": "Graph view",
    "filter_tags": "Filter by tags",
    "no_connections": "No connections"
  },
  "settings": {
    "title": "Settings",
    "language": "Language",
    "ai_provider": "AI Provider",
    "llamacpp_url": "llama.cpp server URL"
  },
  "plugin": {
    "install": "Install",
    "uninstall": "Uninstall",
    "enable": "Enable",
    "disable": "Disable",
    "restart_required": "Restart required"
  }
}
```

```typescript
// i18n/setup.ts
import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

i18n
  .use(initReactI18next)
  .init({
    resources: { en: { translation: enJson }, ru: { translation: ruJson } },
    lng: await tauriApi.settings.getLocale(),  // из config.json
    fallbackLng: 'en',
    interpolation: { escapeValue: false },
  });

// Rust сторона тоже поддерживает локализацию
// для системных уведомлений и сообщений об ошибках
```

---

## 10. Производительность

### Целевые метрики (vault 50k файлов)

| Операция | Целевое время | Как достигается |
|---|---|---|
| Холодный старт приложения | < 3 секунды | Hydrate graph из SQLite в память, lazy load UI |
| Открытие файла | < 50ms | Файл уже на диске, CodeMirror быстрый |
| Full-text поиск | < 100ms | FTS5 с оптимизированными индексами |
| Семантический поиск | < 300ms | sqlite-vec ANN, ~50ms для 500k чанков |
| RAG запрос (до первого токена) | < 1s | Параллельный поиск + быстрый промпт-билд |
| Индексирование одного файла | < 500ms | Параллельный embed + SQLite write |
| Полная реиндексация 50k файлов | < 30 минут | Batch embedding, параллелизм |
| Граф 50k нод (рендер) | < 2s | sigma.js WebGL, прогрессивный рендер |
| Предложения связей | < 2s | Кэш в БД, пересчёт только при изменении |

### Ключевые оптимизации

**1. Граф в памяти:**
```rust
// При старте — один раз загружаем граф из SQLite в petgraph
// Дальше все запросы O(1) - O(V+E) без обращений к диску
// Память: ~100MB для 50k файлов с 500k ссылками
static GRAPH: OnceCell<Arc<GraphStore>> = OnceCell::new();
```

**2. Batch embedding:**
```rust
// Не embed по одному файлу — собираем батчи
let batch_size = 32; // оптимально для llama.cpp
for chunk in files.chunks(batch_size) {
    let embeddings = ai_client.embed_batch(chunk).await?;
    db.insert_embeddings(&embeddings).await?;
}
```

**3. Ленивая загрузка UI:**
```typescript
// Граф грузим только когда пользователь переключается на него
const GraphView = React.lazy(() => import('./components/graph/GraphView'));
```

**4. Виртуализация файлового дерева:**
```typescript
// При 50k файлов файловое дерево нельзя рендерить всё сразу
// Используем @tanstack/react-virtual
import { useVirtualizer } from '@tanstack/react-virtual';
```

**5. Прогрессивная индексация при первом открытии:**
```
Шаг 1 (< 1s):  Построить файловый индекс (имена, размеры, даты)
Шаг 2 (< 10s): Распарсить все .md файлы (ссылки, теги, frontmatter)
Шаг 3 (async): Embed все документы в фоне (пользователь уже работает)
```

---

## 11. Безопасность

### Плагинная безопасность

```
Уровень 1: Permissions система
  └── Плагин объявляет permissions в manifest
  └── Пользователь видит их при установке (как Android)
  └── Runtime проверка перед каждым вызовом API

Уровень 2: WASM sandbox
  └── Нет прямого доступа к файловой системе
  └── Нет сетевых запросов (кроме объявленных)
  └── Memory limit per plugin: 256MB
  └── CPU time limit: 5s per call

Уровень 3: UI isolation
  └── iframe с sandbox="allow-scripts" (без allow-same-origin)
  └── postMessage API с white-list сообщений
  └── CSP заголовки

Уровень 4: Plugin signing (v2.0)
  └── Официальный реестр плагинов с подписями
  └── Community плагины — установка с предупреждением
```

### Данные и приватность

```
- Vault никогда не отправляется в облако без явного действия
- LLM запросы: только к локальному llama.cpp по умолчанию
- При включении облачного LLM: явное предупреждение какие данные уйдут
- API ключи хранятся в OS keychain (не в config.json)
- Логи не содержат контент заметок
```

---

## 12. Фазы разработки

### Фаза 0 — Фундамент (4-6 недель)

**Цель:** Работающий редактор с правильной архитектурой, без AI.

**Deliverables:**
- [ ] Репозиторий: monorepo с pnpm workspaces + Cargo workspace
- [ ] Tauri 2 приложение запускается на Win/Mac/Linux
- [ ] Открытие vault-папки, файловое дерево
- [ ] CodeMirror 6 редактор: markdown, live preview, syntax highlighting
- [ ] `[[wikilinks]]` — парсинг, кликабельность, автокомплит
- [ ] `#tags` — парсинг, фильтрация
- [ ] SQLite схема v1 (без vec): files, links, tags
- [ ] Инкрементальный watcher (notify-rs) + парсер (pulldown-cmark)
- [ ] Граф обратных ссылок в памяти (petgraph)
- [ ] Базовый граф-вью (D3, без WebGL)
- [ ] Full-text поиск (FTS5)
- [ ] i18n RU/EN подключён
- [ ] Базовый plugin loader (manifest + permissions)

**Критерий успеха:** Можно открыть существующий Obsidian vault и работать с ним.

---

### Фаза 1 — AI Core (4-6 недель)

**Цель:** Полноценная AI интеграция с RAG.

**Deliverables:**
- [ ] sqlite-vec подключён, схема обновлена (chunks, vec_chunks)
- [ ] Умный chunker: по заголовкам + sliding window
- [ ] Embedder: запросы к llama.cpp /embedding endpoint
- [ ] Полная (ре)индексация vault с прогресс-баром
- [ ] Инкрементальная индексация при изменении файлов
- [ ] Hybrid search: векторный + FTS5 + RRF fusion
- [ ] Graph boost в RAG
- [ ] Чат-интерфейс в правой панели
- [ ] Streaming ответов через Tauri events
- [ ] Контекст текущего файла в чат
- [ ] Предложения связей (embedding similarity, режим 1)
- [ ] Inline индикатор индексирования
- [ ] OpenAI / Anthropic fallback (настройка в UI)

**Критерий успеха:** Можно спросить "что я знаю о X?" и получить ответ с источниками.

---

### Фаза 2 — Plugin Ecosystem (3-4 недели)

**Цель:** Плагинная система готова к сторонним разработчикам.

**Deliverables:**
- [ ] WASM sandbox (Wasmtime)
- [ ] Plugin API v1.0: vault, ai, ui, settings
- [ ] nexus-plugin-sdk npm пакет опубликован
- [ ] Документация API + примеры плагинов
- [ ] UI точки расширения: sidebar, status-bar, commands, context-menu
- [ ] CodeMirror extension API для плагинов
- [ ] Plugin marketplace UI (поиск, установка, обновление)
- [ ] First-party плагины как референс:
  - `nexus-daily-notes`
  - `nexus-templates`
  - `nexus-ai-suggest` (LLM reasoning, режим 2)
  - `nexus-graph-view-advanced` (sigma.js WebGL, кластеризация)

**Критерий успеха:** Сторонний разработчик может написать плагин по документации.

---

### Фаза 3 — Git Sync + Performance (3-4 недели)

**Цель:** Надёжная синхронизация и производительность на 50k файлов.

**Deliverables:**
- [ ] `nexus-git-sync` плагин (auto-commit, pull, push)
- [ ] Conflict detection и UI для разрешения
- [ ] Умные commit-сообщения
- [ ] Нагрузочное тестирование: 50k файлов
- [ ] Оптимизация холодного старта (< 3s)
- [ ] sigma.js WebGL граф (100k нод)
- [ ] Виртуализация файлового дерева
- [ ] Batch embedding оптимизация
- [ ] Профилирование и устранение узких мест

**Критерий успеха:** Vault 50k файлов открывается за 3 секунды, поиск < 100ms.

---

### Фаза 4 — Polish + Mobile Prep (2-3 недели)

**Цель:** Production-ready десктоп, фундамент для мобилки.

**Deliverables:**
- [ ] Тема оформления (светлая/тёмная + кастомные CSS переменные)
- [ ] Keyboard shortcuts (vim mode опционально, через плагин)
- [ ] PDF / Print экспорт
- [ ] Crash reporter (анонимный)
- [ ] Auto-updater (Tauri updater)
- [ ] Onboarding для новых пользователей
- [ ] `packages/nexus-md-parser` выделен как shared пакет
- [ ] Sync протокол задокументирован (для будущего мобильного клиента)
- [ ] Mobile app структура создана (`apps/mobile/`)

**Критерий успеха:** Готово к публичному beta-релизу.

---

## 13. Риски и решения

| Риск | Вероятность | Влияние | Решение |
|---|---|---|---|
| sqlite-vec недостаточно быстр для 50k файлов | Средняя | Высокое | Benchmark на фазе 1. Fallback: usearch embedded или Qdrant |
| llama.cpp API меняется | Низкая | Среднее | Provider abstraction изолирует изменения |
| WASM для плагинов слишком медленный | Средняя | Среднее | JS плагины как fallback. Native для критичных |
| Конфликты git при активной мобильной работе | Высокая | Среднее | Three-way merge + UI для разрешения |
| CodeMirror 6 плохо работает с 1MB+ файлами | Низкая | Высокое | Chunk rendering, только видимая область |
| petgraph in-memory граф слишком большой | Низкая | Высокое | При > 200k файлов — SQLite только, убрать из памяти |
| Экосистема плагинов не появится | Высокая | Среднее | First-party плагины как демонстрация возможностей |

---

## Приложение A: Команды для старта

```bash
# Клонируем и настраиваем
git clone https://github.com/your-org/nexus
cd nexus
pnpm install
cargo build

# Запускаем dev окружение
pnpm dev         # Tauri desktop в dev режиме

# Тесты
pnpm test        # Vitest frontend
cargo test       # Rust unit tests

# Проверяем что llama.cpp сервер доступен
curl http://localhost:8080/health

# Первая индексация vault
# (через UI: File → Open Vault → выбрать папку)
```

## Приложение B: Версионирование Plugin API

```
v1.0 → vault CRUD, ai.embed, ai.complete, базовый UI
v1.1 → graph API (read-only), ai.searchSemantic
v1.2 → vault:watch события, editor extensions
v2.0 → native плагины, полный graph write API, collaboration
```

---

*Документ живой. Обновляется по мере принятия архитектурных решений.*  
*Последнее обновление: 2026*
