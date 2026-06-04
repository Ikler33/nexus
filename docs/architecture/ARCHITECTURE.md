# Архитектурный план: LLM-Native Knowledge Base
> Obsidian-форк с глубокой интеграцией локальных LLM  
> Версия документа: 1.1 | Статус: Living Document
> Ревизия 1.1 — правки по итогам архитектурного ревью (`REVIEW.md`). Снимок до правок: `ARCHITECTURE-v1.0-backup.md`. Зафиксированные решения — в разделе 0.

---

## Оглавление

0. [Журнал архитектурных решений (ADR) — v1.1](#0-журнал-архитектурных-решений-adr)
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

## 0. Журнал архитектурных решений (ADR)

> Зафиксировано по итогам ревью (`REVIEW.md`). Эти решения — основа правок v1.1; разделы ниже приведены в соответствие с ними.

### ADR-001 · Модель плагинов: JS-first + host-broker
**Решение.** Основной тип плагина — доверенный JS/TS-модуль с прямым типизированным доступом к app API и CodeMirror 6. Изоляция логики — в Web Worker; редакторные расширения исполняются в main-контексте редактора. WASM — опциональный под-формат для тяжёлых чистых вычислений, вызываемый изнутри JS-плагина. Sandbox-iframe — только для тяжёлого изолированного UI-вью, без прямого доступа к CodeMirror.
**Почему.** «Нужна экосистема» — non-negotiable; WASM-first максимизирует порог входа и делает живые CM-расширения/колбэки невыразимыми. JS-first — проверенный путь Obsidian/VS Code.
**Каскад.** §3, §4.4, §7 (целиком), §11, §12, §13, Приложение B.

### ADR-002 · Безопасность: capability-broker + path-scoped permissions
**Решение.** Граница безопасности — это **host-side capability broker**, а не iframe-sandbox. Path-scoped права (`vault:read:Work/**`), runtime-grant для чувствительного, обязательный неотключаемый audit-log. На каждый плагин — отдельный `MessagePort` (identity по порту, не по `pluginId` из payload). Подпись плагинов — в Фазу 2 (вместе с marketplace). Плагины **не** хранятся в git: синхронизируется только декларация `id@version#sha256` + настройки. At-rest шифрование (SQLCipher) — опциональный тоггл, не обязателен в v1.
**Почему.** «Privacy by default» для личной базы; закрывает capability-laundering, confused-deputy и git-доставку неподписанного кода.
**Каскад.** §11 (+ новый «Модель угроз»), §8.1, §7.2.

### ADR-003 · БД: rusqlite + выделенный write-actor
**Решение.** `rusqlite` вместо `sqlx`. Запись — через единый write-поток (mpsc-actor) синхронными транзакциями `conn.transaction(|c| …)`. Чтение — пул read-коннектов (WAL допускает параллельное чтение). `load_extension` для векторного ANN выполняется один раз на write-коннекте.
**Почему.** SQLite — single-writer; async-замыкание-транзакция `sqlx` невыразима в стабильном Rust; async поверх единственного писателя — оверхед.
**Каскад.** §3, §4.2, §5.

### ADR-004 · Граф: источник истины — SQLite, petgraph — опц. кэш
**Решение.** Беклинки и обходы — запросами к SQLite по индексу `idx_links_target`. `petgraph` в памяти — опциональный кэш только для тяжёлых графовых алгоритмов (на старте не требуется).
**Почему.** Убирает дублирование/рассинхрон состояния, фантомные метрики «50ms/100MB» и hydrate из бюджета холодного старта.
**Каскад.** §4.2, §10.

### ADR-005 · AI-провайдеры: раздельные Chat и Embedding
**Решение.** Два трейта — `ChatProvider` и `EmbeddingProvider` — с раздельными `chat_url`/`embedding_url` и моделями. Облачный fallback — только по явному opt-in именно на fallback, с индикацией «отвечает облако». Эмбеддер — **мультиязычный** (bge-m3 / multilingual-e5) под требование RU/EN.
**Почему.** У Anthropic нет embeddings-эндпоинта; один инстанс llama.cpp не обслуживает эффективно и генерацию 27B, и батч-эмбеддинг (опыт SA Agent: разнесено по хостам); кросс-язычный RAG требует мультиязычной модели.
**Каскад.** §4.3, §5 (config), §6.

### ADR-006 · Дизайн-система «Hermes»: токены + OKLCH + self-hosted шрифты (Фаза 4)
**Решение.** Принять подготовленную дизайн-систему **Hermes** (`docs/design/handoff/`) как источник истины визуала. Токенный слой (`src/styles.css`) — **OKLCH**-палитра, темы через `data-theme` (light «old paper» / dark «warm clay»), акцент через `data-accent` (amber/teal/sage/clay). Шрифты (**Onest** UI / **JetBrains Mono** mono / **Source Serif 4** проза) — **self-hosted** через `@fontsource` (offline/local-first, без Google Fonts в рантайме; CSP `font-src 'self' data:` уже разрешает). `.jsx/.html` прототипы — **референс**, не прод-код: пересоздаём на наших настоящих компонентах (CM6/sigma/i18n/broker). Порядок Фазы 4: **токены-фундамент → порескринный рестайл существующих экранов → новые экраны**.
**Почему.** Архитектура уже токенная (CSS-переменные) и имена токенов совпадают → дизайн ложится как **фундамент**, а не ретрофит, и существующий апп перекрашивается когерентно «за один ход». OKLCH даёт перцептивно-ровные темы и расчётные акценты. Self-host шрифтов — offline/privacy (local-first).
**Каскад.** §2 (`docs/design/`), §9 (i18n тем/языка persist), §12 (Фаза 4 + новые экраны Home/tweaks/conflict), DESIGN, `docs/dev/design.md`.

### Механические правки (без развилок — внесены по тексту)
Эмбеддинг по чанкам, а не по файлу (§4.2/§6.1) · динамическая размерность эмбеддера вместо хардкода 1024 (§5) · FTS5 поверх `chunks` + триггеры синхронизации (§5) · каскадная очистка векторного индекса при удалении/реиндексации (§5) · RRF без аддитивного `+0.2`, граф как отдельный ранг (§6.2) · overlap внутри окна + токенайзер эмбеддера для подсчёта (§6.1) · префиксы `search_query/document` + L2-нормализация (§6.2) · rerank отдельным reranker-инстансом или LLM-listwise, без мифа «<200ms» (§6.2) · стриминг через `Channel` + финализация в историю + отмена (§4.1) · reconcile по пути при atomic-save + ignore-список в watcher + единый debounce (§4.2) · модель групп/вкладок вместо одиночного `currentFile` (§4.1) · пагинация/бинарный канал для тяжёлых IPC (§4.1) · русские плюралы + `Intl` + `Collator`, i18n бэкенда и плагинов (§9) · eval-харнесс RAG (§6/§12) · восстановление индекса после краха, миграции схемы, обработка вложений и не-md (новые подразделы).

---

## 1. Обзор и принципы

### Название проекта
**Nexus** — локально-первый knowledge base с LLM-нативной архитектурой.

### Ключевые принципы (non-negotiable)

| Принцип | Следствие |
|---|---|
| **Local-first** | Всё работает без интернета. Облако — опция |
| **Plain files** | Vault = папка с `.md` файлами. Совместимость с Obsidian |
| **Plugin-first** | Необязательные фичи — плагины (JS-first). Привилегированные системные фичи (граф, git-sync) — core modules, не песочные плагины |
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
│   │       │       ├── broker.rs   # Capability-broker — граница прав (ADR-002)
│   │       │       ├── wasm.rs      # ОПЦ. Wasmtime для тяжёлых вычислений
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
│   ├── nexus-graph-view/           # Граф — core module (привилегированный, не sandbox-плагин)
│   ├── nexus-daily-notes/
│   ├── nexus-templates/
│   ├── nexus-git-sync/             # Git sync — core module (нужны сеть/FS вне песочницы)
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
| Graph | **sigma.js 3 + graphology** | Единый движок (WebGL) для любого размера — без второй D3-реализации; layout считается в Web Worker. Для больших vault основной режим — локальный N-hop граф |
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
| Database | **SQLite via rusqlite** | Embedded, single-writer. Запись — выделенный write-actor (синхронные транзакции), чтение — пул read-коннектов (WAL). См. ADR-003 |
| Vector ANN | **usearch** (embedded, HNSW, mmap) | Настоящий ANN сразу, не «потом». `sqlite-vec` — опция для точного KNN на малых vault; HNSW в нём пока нет (это flat-скан) |
| Git | **git2-rs** (в `spawn_blocking`) | libgit2 синхронный — все вызовы только во write-actor/`spawn_blocking`, не на async-рантайме |
| HTTP client | **reqwest** | Async; раздельные клиенты к chat- и embedding-серверам, с таймаутами и retry |
| Plugin runtime | **JS Worker + main-thread мост** | Логика — в Worker; редакторные расширения — в main-контексте. Опц. **Wasmtime** для тяжёлых чистых вычислений изнутри JS-плагина. См. ADR-001 |
| Serialization | **serde + serde_json** | |
| Async runtime | **Tokio** | |
| Logging | **tracing** | Структурированные логи |

### AI инфраструктура

| Компонент | Технология |
|---|---|
| LLM inference (chat) | **llama.cpp HTTP server**, Qwen3 27B — отдельный `chat_url` |
| Embeddings | **Отдельный** embedding-сервер (`embedding_url`), модель **мультиязычная** (bge-m3 / multilingual-e5) под RU/EN. Раздельно от chat — см. ADR-005 |
| RAG framework | Нативная реализация на Rust (без Python зависимостей) |
| Vector search | **usearch** (HNSW); метаданные/фильтры — в SQLite |
| Reranking | Отдельный reranker-инстанс (bge-reranker) **или** LLM-listwise — НЕ через chat-модель «<200ms» |
| Chunking | Кастомный chunker на Rust с учётом markdown-структуры; подсчёт токенов — токенайзером эмбеддера |
| Облачный fallback | OpenAI / Anthropic — **только chat**, по явному opt-in (у Anthropic нет embeddings) |

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
// stores/workspace.ts — вкладки и сплиты (вместо одиночного currentFile).
// Заложено в Фазе 0: ретрофит на вкладки позже = переписывание ядра.
type BufferId = string;
interface Buffer { id: BufferId; path: string; doc: string; dirty: boolean; }
interface EditorGroup { id: string; tabs: BufferId[]; activeTab: BufferId | null; }
interface WorkspaceStore {
  buffers: Map<BufferId, Buffer>;
  groups: EditorGroup[];               // несколько сплитов, в каждом свои вкладки
  activeGroupId: string;
  openFile: (path: string, group?: string) => Promise<void>;
  saveBuffer: (id: BufferId) => Promise<void>;
  // Активный документ = groups[activeGroupId].activeTab — на него завязаны
  // AI-контекст, backlinks, suggest (а не на глобальный currentFile).
}

// stores/graph.ts — источник истины граф = SQLite (ADR-004); на фронте не держим
interface GraphStore {
  view: 'local' | 'global';
  loadLocal: (center: string, hops: number) => Promise<GraphData>;  // основной режим
  filters: GraphFilter;
}

// stores/ai.ts — стриминг скоупится по сессии (см. useChat ниже)
interface ChatSession { id: string; messages: ChatMessage[]; status: 'idle'|'streaming'|'error'; }
interface AIStore {
  sessions: Map<string, ChatSession>;
  activeSessionId: string | null;
  isIndexing: boolean;
  indexProgress: IndexProgress | null;
  appendMessage: (sessionId: string, m: ChatMessage) => void;  // финализация ответа
  startChat: (sessionId: string, msg: string, context: RAGContext) => Promise<void>;
  cancelChat: (sessionId: string) => Promise<void>;
  suggestions: LinkSuggestion[];
}

// stores/plugin.ts
interface PluginStore {
  installed: PluginManifest[];
  enabled: Set<string>;
  togglePlugin: (id: string) => Promise<void>;   // hot enable/disable, без рестарта
}
```

**Tauri IPC — все команды типизированы:**

```typescript
// lib/tauri-api.ts — единственное место где вызываем invoke
export const tauriApi = {
  vault: {
    readFile: (path: string) => invoke<string>('read_file', { path }),
    writeFile: (path: string, content: string) => invoke<void>('write_file', { path, content }),
    listDir: (dirPath: string) => invoke<FileEntry[]>('list_dir', { dirPath }),   // лениво по папке — НЕ 50k одним invoke
    searchFullText: (query: string) => invoke<SearchResult[]>('search_full_text', { query }),  // фильтр дерева — тоже здесь (FTS), не в JS
  },
  graph: {
    // полный граф не отдаём мегабайтным invoke — только локальный/постранично
    getLocalGraph: (center: string, hops: number) => invoke<GraphData>('get_local_graph', { center, hops }),
    getBacklinks: (path: string) => invoke<BacklinkEntry[]>('get_backlinks', { path }),  // SQLite WHERE target_id=?
  },
  ai: {
    searchSemantic: (query: string, topK: number) => invoke<SemanticResult[]>('semantic_search', { query, topK }),
    getSuggestions: (path: string) => invoke<LinkSuggestion[]>('get_link_suggestions', { path }),
    // Чат через per-session Channel (НЕ глобальный event): канал привязан к вызову,
    // сам решает корреляцию и порядок токенов
    startChat: (sessionId: string, messages: ChatMessage[], context: string[], onChunk: Channel<StreamEvent>) =>
      invoke<void>('start_chat_stream', { sessionId, messages, context, onChunk }),
    cancelChat: (sessionId: string) => invoke<void>('cancel_chat', { sessionId }),
  },
  git: {
    getStatus: () => invoke<GitStatus>('git_status'),
    commit: (message: string) => invoke<void>('git_commit', { message }),
    pull: () => invoke<PullResult>('git_pull'),
    push: () => invoke<void>('git_push'),
  },
};
```

**Streaming LLM ответов через per-session `Channel` (Tauri 2):**

```typescript
// hooks/useChat.ts — стрим скоупится по сессии; токены копятся ВНЕ React-стейта;
// на 'done' сообщение финализируется в историю; есть отмена.
export function useChat(sessionId: string) {
  const bufferRef = useRef('');                          // аккумулятор — нет O(n²) ре-рендеров
  const [text, setText] = useState('');                  // throttled-снапшот для UI
  const [status, setStatus] = useState<'idle'|'streaming'|'done'|'error'>('idle');

  const send = async (messages: ChatMessage[], context: string[]) => {
    bufferRef.current = ''; setText(''); setStatus('streaming');
    const channel = new Channel<StreamEvent>();          // привязан к ЭТОМУ вызову
    let raf = 0;
    channel.onmessage = (ev) => {
      switch (ev.kind) {
        case 'token':
          bufferRef.current += ev.text;
          if (!raf) raf = requestAnimationFrame(() => { setText(bufferRef.current); raf = 0; });
          break;
        case 'done':                                      // финализация: ответ → история (+ chat_messages)
          useAIStore.getState().appendMessage(sessionId, { role: 'assistant', content: bufferRef.current });
          setStatus('done'); break;
        case 'error': setStatus('error'); break;
      }
    };
    await tauriApi.ai.startChat(sessionId, messages, context, channel);
  };

  const cancel = () => tauriApi.ai.cancelChat(sessionId); // Rust: CancellationToken + tokio::select!
  return { text, status, send, cancel };
}
```

> Глобальный `listen('llm-stream')` без `sessionId` смешивал бы токены двух сессий (чат + автосаммари + suggest пишут в один буфер) и не финализировал ответ в историю. `Channel` на вызов + типизированные `kind: token|usage|done|error` (как уже в SSE SA Agent по `jobId`) закрывают это by design. Markdown рендерится по throttled-снапшоту, а не на каждый токен.

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
              ┌───────▼────────────┐
              │ rusqlite (write-    │
              │ actor) + usearch    │
              │ (ANN, sibling-файл) │
              └─────────────────────┘
```

**Vault Manager** — сердце системы:

```rust
// vault/watcher.rs — debounce НЕ встроен в notify: используем notify-debouncer-full.
use notify_debouncer_full::{new_debouncer, DebounceEventResult};

pub struct VaultWatcher { debouncer: Debouncer<RecommendedWatcher, RecommendedCache> }

pub enum VaultEvent {
    Upsert(PathBuf),                 // Created/Modified/целевая часть Rename → единый путь
    Deleted(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

// Игнор ОБЯЗАТЕЛЕН: notify не читает .gitignore, а nexus.db лежит ВНУТРИ vault и
// постоянно пишет .db-wal/-shm → рекурсивный watcher словит свои же записи (цикл реиндексации).
fn is_ignored(p: &Path) -> bool {
    p.components().any(|c| matches!(c.as_os_str().to_str(), Some(".nexus") | Some(".git")))
        || p.extension().and_then(|e| e.to_str()).map_or(false, |e| e == "db" || e.starts_with("db-"))
        || p.file_name().and_then(|n| n.to_str()).map_or(false, |n| n.starts_with('.') || n.ends_with(".conflict"))
}

impl VaultWatcher {
    pub fn new(vault_path: &Path, tx: mpsc::Sender<VaultEvent>) -> Result<Self> {
        // ОДНО окно дебаунса (в v1.0 было противоречие 300 vs 500мс). Схлопывает шторм
        // и пару remove+create от atomic-save редактора (tmp→rename) в одно событие по пути.
        let mut debouncer = new_debouncer(Duration::from_millis(400), None, move |res: DebounceEventResult| {
            // отфильтровать is_ignored → нормализовать в VaultEvent ПО ПУТИ (не по событию)
        })?;
        debouncer.watch(vault_path, RecursiveMode::Recursive)?;
        Ok(Self { debouncer })
    }
}

// Reconcile по пути — гарантирует стабильность file_id при atomic-save и переименованиях:
async fn on_event(&self, ev: VaultEvent) -> Result<()> {
    match ev {
        // upsert по НОРМАЛИЗОВАННОМУ пути: есть запись → UPDATE (file_id сохраняется), иначе INSERT.
        // Atomic-save (tmp→rename целевого) приходит как Upsert(target) — НЕ delete+create.
        VaultEvent::Upsert(p)        => self.reindex_file(&normalize(&p)).await,
        // move записи на новый путь БЕЗ потери file_id → ссылки/беклинки целы; обновить target_raw
        // ссылок на старое имя (Obsidian-совместимо).
        VaultEvent::Renamed{from,to} => self.writer.transaction(move |tx| rename_file(tx,&from,&to)).await,
        VaultEvent::Deleted(p)       => self.writer.transaction(move |tx| soft_delete(tx,&p)).await,
    }
}
// Echo-suppression: собственные записи индексатора/плагинов помечаются; событие по только что
// записанному хэшу игнорируется (плюс is_ignored для .nexus/.git/*.db*) — нет цикла реиндексации.

// Нормализация путей и резолвинг wikilink (Obsidian-совместимость):
// - пути в БД нормализованы (NFC, разделитель '/'); сравнение case-insensitive на macOS/Windows,
//   case-sensitive на Linux — политика фиксируется на vault при создании.
// - UNIQUE(path) поверх нормализации, чтобы 'My Note.md' и 'my note.md' не плодили дубли.
// - [[wikilink]] резолвится по имени/алиасу (таблица aliases) + кратчайший уникальный путь;
//   неоднозначность → выбор по близости папки.

// Паники/отмена на IPC-границе: каждая Tauri-команда и каждый вызов плагина изолированы
// (catch_unwind / trap не роняет рантайм → возвращается ошибка); долгие операции — отменяемы
// (CancellationToken + tokio::select!), блокирующая работа (git2, парсинг) — в spawn_blocking.

// vault/indexer.rs — инкрементальный индексатор
pub struct VaultIndexer {
    writer: WriteActor,         // ADR-003: единый write-поток, синхронные транзакции rusqlite
    reader: ReadPool,           // пул read-коннектов (WAL)
    embedder: Arc<dyn EmbeddingProvider>,  // ADR-005: отдельный embedding-провайдер
    embed_sem: Arc<Semaphore>,  // ограничение конкурентности к embedding-серверу
}

impl VaultIndexer {
    // Реагирует на VaultEvent::Upsert; reconcile по пути сохраняет file_id (см. watcher).
    pub async fn reindex_file(&self, path: &Path) -> Result<()> {
        // 1. Дешёвый шорткат по mtime+size из БД — НЕ читаем тысячи файлов ради хэша.
        let meta = tokio::fs::metadata(path).await?;
        if self.reader.unchanged_by_mtime_size(path, &meta).await? { return Ok(()); }

        let content = tokio::fs::read_to_string(path).await?;
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
        if self.reader.get_file_hash(path).await?.as_deref() == Some(&hash) { return Ok(()); }

        // 2. parse → chunk. Embed ЗАВИСИТ от чанков, поэтому НЕ join! с parse (в v1.0
        //    был ложный tokio::join). CPU-bound парсинг/чанкинг → spawn_blocking.
        let parsed = tokio::task::spawn_blocking({
            let c = content.clone(); move || parse_and_chunk(&c)
        }).await??;

        // 3. Эмбеддим КАЖДЫЙ чанк (не файл целиком!), батчем, под семафором к серверу.
        let _permit = self.embed_sem.acquire().await?;
        let texts: Vec<&str> = parsed.chunks.iter().map(|c| c.content.as_str()).collect();
        let embeddings: Vec<Vec<f32>> = self.embedder.embed_documents(&texts).await?;  // 1:1 к чанкам

        // 4. Атомарно во write-actor: синхронная rusqlite-транзакция (ADR-003).
        self.writer.transaction(move |tx| {
            let file_id = upsert_file(tx, path, &hash, &meta, &parsed.frontmatter)?;  // СОХРАНЯЕТ file_id
            update_links(tx, file_id, &parsed.outgoing_links)?;
            update_tags(tx, file_id, &parsed.tags)?;
            replace_chunks(tx, file_id, &parsed.chunks)?;                 // DELETE старых + INSERT
            replace_vectors(tx, file_id, &parsed.chunks, &embeddings)?;   // + чистка ANN от старых векторов
            Ok(())
        }).await
    }
}
```

**Graph Store** — граф в памяти для быстрых запросов:

```rust
// graph/store.rs — ADR-004: источник истины графа = SQLite.
// petgraph как источник истины УБРАН: дублирование с таблицей links, рассинхрон трёх
// независимых локов (RwLock + 2×DashMap), фантомные «50ms/100MB» и hydrate в бюджете старта.

// Беклинки — запросом по индексу idx_links_target: доли мс из page-cache, без диска,
// без рассинхрона, НЕ блокирует холодный старт.
pub async fn get_backlinks(reader: &ReadPool, file_id: FileId) -> Result<Vec<Backlink>> {
    reader.query(
        "SELECT source_id, context, line_number FROM links WHERE target_id = ?1",
        file_id,
    ).await
}

// Граф-вью тянет локальный N-hop через get_local_graph (рекурсивный CTE по links),
// а не весь граф в память. Опциональный in-memory кэш заводим ТОЛЬКО под тяжёлые
// алгоритмы (PageRank/кластеризация), которых сейчас в требованиях нет; и тогда —
// единая структура под ОДНИМ локом или arc_swap-снапшот для lock-free чтения.
pub struct GraphCache { state: arc_swap::ArcSwap<GraphState> }  // опционально, не на старте
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

> ⚠️ **Статус реализации (2026-06, фактчек кросс-плана):** трейты `ChatProvider`/`EmbeddingProvider`
> реализованы (`ai/chat.rs`, `ai/embedder.rs`). Агрегатор **`AIClient` ниже — ЦЕЛЕВОЙ дизайн, в коде
> ОТСУТСТВУЕТ** (`AIClient`/`chat_fallback`/`cloud_fallback`/`guard_first_token`/`complete_json` — 0
> совпадений). Сейчас `commands/chat.rs` зовёт `ChatProvider` напрямую; единого composition-root и
> cloud-fallback нет. Реализация агрегатора + единый egress-хелпер — `CROSSCUT_PLAN.md` #5/#16.

```rust
// ai/provider.rs — РАЗДЕЛЁННЫЕ трейты (ADR-005): chat и embeddings — разные сущности
// с разными моделями/хостами/жизненным циклом. У Anthropic embeddings нет вовсе.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    async fn complete(&self, messages: &[Message], opt: &CompletionOptions)
        -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>>;
    fn max_context_tokens(&self) -> usize;
    fn name(&self) -> &str;
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    // query/document асимметрия (nomic/bge: разные префиксы); L2-нормализация ВНУТРИ
    async fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
    fn dim(&self) -> usize;          // размерность ИЗ модели, не хардкод 1024
    fn model_id(&self) -> &str;      // для инвалидции векторов при смене модели
}

// ai/client.rs — раздельные клиенты к chat_url и embedding_url
pub struct AIClient {
    chat: Arc<dyn ChatProvider>,                    // llama.cpp Qwen3 по умолчанию
    chat_fallback: Option<Arc<dyn ChatProvider>>,   // облако — ТОЛЬКО chat
    cloud_fallback_enabled: bool,                   // явный opt-in именно на fallback
    embedder: Arc<dyn EmbeddingProvider>,           // отдельный хост; в облако НЕ фоллбэчит
}

impl AIClient {
    pub async fn complete(&self, msgs: &[Message], opt: &CompletionOptions)
        -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>>
    {
        // Фоллбэк — ТОЛЬКО до первого выданного токена и ТОЛЬКО при явном opt-in:
        // иначе сбой локального сервера = тихая отправка vault в облако (нарушение privacy).
        // Обрывы ВНУТРИ стрима ловит обёртка guard_first_token, а не только этап коннекта.
        match self.chat.complete(msgs, opt).await {
            Ok(stream) => Ok(guard_first_token(stream)),
            Err(e) if self.cloud_fallback_enabled => {
                warn!("chat primary failed before stream: {e}; cloud fallback (opt-in, индикация в UI)");
                self.chat_fallback.as_ref().ok_or(e)?.complete(msgs, opt).await
            }
            Err(e) => Err(e),
        }
    }
}
```

---

### 4.4 Plugin System

Плагинная система — это отдельная подсистема. Подробно описана в [разделе 7](#7-плагинная-система-подробно).

**Типы плагинов (ADR-001 — JS-first):**

```
1. JS/TS-плагин (основной) — доверенный модуль, прямой типизированный доступ к app и
                             CodeMirror 6. Логика — в Web Worker; редакторные расширения
                             (декорации/виджеты/autocomplete) — в main-контексте.
                             Низкий порог входа = живая экосистема (путь Obsidian/VS Code).
2. UI-вью в iframe         — ТОЛЬКО тяжёлый изолированный UI без прямого доступа к CM6.
3. WASM-модуль (опц.)      — чистые тяжёлые вычисления, вызывается ИЗНУТРИ JS-плагина.
4. Native / core modules   — привилегированные Rust-крейты первой стороны (git-sync,
                             индексатор). Это НЕ плагины экосистемы — отдельная категория.
```

> Граница безопасности — **host-side capability broker** (ADR-002), а не sandbox iframe.
> Матрица «тип × возможности» — в §7.7.

---

### 4.5 Sync Layer

Git-based синхронизация описана в [разделе 8](#8-git-sync-протокол).

---

### 4.6 Command Registry и клавиатурная модель

Единый реестр команд: ядро И плагины регистрируют команды в один registry; Command Palette
(Cmd/Ctrl+P), context-menu и keymap работают поверх него. Спроектирован уже в Фазе 0 — иначе
плагинный `registerCommand` (Фаза 2) проектировался бы вслепую.

```typescript
interface Command {
  id: string;                       // "vault.newNote", "ai.askAboutFile"
  title: string;                    // i18n-ключ
  when?: Context;                   // условие доступности (есть активный файл и т.п.)
  run: (ctx: CommandCtx) => void | Promise<void>;
  defaultKey?: string;              // "mod+n"
}
interface CommandRegistry {
  register(cmd: Command): Disposable;   // и ядро, и плагины — один путь
  run(id: string, ...args: unknown[]): Promise<void>;
  keymap: Keymap;                       // пользовательский ремап
}
```

Разрешение конфликтов хоткеев — по приоритету **пользователь > плагин > ядро**, с уведомлением о перекрытии. Все действия достижимы с клавиатуры (принцип keyboard-first, см. `DESIGN.md`).

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

-- Full-text search (FTS5) — поверх chunks.content.
-- В files НЕТ колонки content, поэтому external-content FTS на files (как в v1.0) был СЛОМАН.
-- Body-поиск идёт по чанкам (и удобен для jump-to-section), title-поиск — по files.title.
CREATE VIRTUAL TABLE fts_chunks USING fts5(
    content,
    content=chunks,
    content_rowid=id
);
-- ОБЯЗАТЕЛЬНЫЕ триггеры синхронизации external-content FTS (без них — рассинхрон):
CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
  INSERT INTO fts_chunks(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
  INSERT INTO fts_chunks(fts_chunks, rowid, content) VALUES ('delete', old.id, old.content);
END;
CREATE TRIGGER chunks_au AFTER UPDATE ON chunks BEGIN
  INSERT INTO fts_chunks(fts_chunks, rowid, content) VALUES ('delete', old.id, old.content);
  INSERT INTO fts_chunks(rowid, content)               VALUES (new.id, new.content);
END;

-- Векторный ANN — usearch (HNSW), отдельный файл .nexus/vectors.usearch, ключ = chunk_id.
-- Размерность = embedder.dim() (НЕ хардкод 1024; default-модель multilingual-e5/bge = 768/1024
-- в зависимости от выбора). sqlite-vec в v1.0 назывался «HNSW», но это flat-скан — заменён.
-- Удаление/реиндексация чанков чистит ANN в той же write-транзакции (replace_vectors).
-- Префильтр по папке/тегу/дате — через SQLite ДО KNN.
-- Метаданные модели хранятся в settings: 'embedding.model', 'embedding.dim',
-- 'embedding.version'. Смена любого → полная переэмбеддизация (см. §6.5).

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
│   ├── vectors.usearch         # ANN-индекс usearch (в .gitignore)
│   ├── config.json             # Несекьюрные настройки vault (в git)
│   ├── local.json              # Endpoints/remote — секьюрно, НЕ в git (ADR-002)
│   ├── plugins/                # Установленные плагины — НЕ в git (только декларация в config.json)
│   │   └── plugin-name/
│   └── themes/                 # Темы оформления (в git)
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
  "version": "1.1",
  "vault_name": "My Knowledge Base",
  "ai": {
    "_note": "endpoints / ключи / fallback — НЕ здесь (не в git), а в .nexus/local.json + keychain (ADR-002/СП-7)",
    "auto_suggest_links": true,
    "suggest_on_save": false,
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

**Локальные настройки (`.nexus/local.json` — в `.gitignore`, НЕ синхронизируются; ADR-002/СП-7):**

```json
{
  "ai": {
    "chat":      { "url": "http://localhost:8080", "model": "qwen3-27b", "context_window": 32768 },
    "embedding": { "url": "http://localhost:8081", "model": "multilingual-e5-large", "dim": 1024 },
    "reranker":  { "url": "http://localhost:8082", "enabled": false },
    "cloud_fallback": { "enabled": false, "provider": null }
  },
  "sync": { "remote": null, "auto_pull": false, "auto_commit_idle_sec": 60 }
}
```
> API-ключи — в OS keychain, не в файлах. `*.url` валидируются (анти-SSRF, §11): по умолчанию только loopback. Раздельные хосты chat/embedding/reranker — ADR-005.

### 5.1 Миграции схемы и восстановление после краха

**Миграции.** Версионированные SQL-файлы в `db/migrations/`; раннер на старте применяет недостающие в транзакции, версия — в **`PRAGMA user_version`** (реализация Ф0-2: транзакционно, без chicken-egg с таблицей `settings`, без гонок; см. `docs/dev/db.md`). `settings` остаётся для прикладных настроек.
- Обычные таблицы — `ALTER`/`CREATE`.
- `fts_chunks` (FTS5) и usearch **нельзя `ALTER`** — пересоздаются и переиндексируются из `chunks` (контент-таблица = источник). Фоновая операция с прогрессом. ⚠️ **Статус (2026-06):** примитива пересборки в раннере миграций ПОКА НЕТ (`db/migrations.rs` — forward-only `execute_batch` + `user_version`, без `rebuild_derived`/reindex-хука). Реализовать ДО первой схемо-миграции `chunks` — `CROSSCUT_PLAN.md` #13 (жёсткая зависимость).
- Смена модели эмбеддера — отдельный путь (§6.5): пересоздание usearch под новую размерность + переэмбеддизация.

**Восстановление.** Индексация атомарна на файл (одна write-транзакция), но прогон по vault может прерваться (краш/убийство процесса). На старте — reconcile-проход:
- `files`, где `indexed_at < updated_at` ИЛИ `hash` на диске ≠ `hash` в БД → в очередь реиндексации;
- файлы, удалённые на диске, но не помеченные → soft-delete;
- очередь индексации **персистится** и докручивается в фоне, переживая рестарт.

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
                  │ POST embedding_url│
                  │ per-chunk, batch │
                  │ (отд. хост,ADR-005)│
                  └────────┬─────────┘
                           │
                           ▼
                  ┌──────────────────┐
                  │  Write-actor TX  │
                  │ (rusqlite, ADR-3)│
                  │                  │
                  │ - files          │
                  │ - links          │
                  │ - tags           │
                  │ - chunks (+FTS)  │
                  │ - usearch ANN    │
                  └──────────────────┘
```

**Умный чанкер — ключевой компонент:**

```rust
// ai/chunker.rs — токены считаем токенайзером ЭМБЕДДЕРА (не эвристикой: для кириллицы
// ошибка 1.5–2×). Frontmatter вырезан из тела (зашумляет вектор). Fenced-code и таблицы
// не рвём посреди окна.
pub struct MarkdownChunker {
    tokenizer: EmbedTokenizer,   // словарь модели эмбеддера (через /tokenize или GGUF)
    max_tokens: usize,           // 512 — ВКЛЮЧАЯ overlap
    overlap_tokens: usize,       // 64
}

impl MarkdownChunker {
    pub fn chunk(&self, parsed: &ParsedDocument) -> Vec<Chunk> {
        // frontmatter уже отделён парсером и в тело НЕ попадает (лежит в files.frontmatter)
        let sections = self.split_by_headings(parsed);   // code-block/таблица — атомарны

        let mut chunks = Vec::new();
        for section in &sections {
            if self.tokenizer.count(&section.content) <= self.max_tokens {
                chunks.push(section.as_chunk());
            } else {
                // overlap ВНУТРИ окна (448 нового + 64 хвоста = 512), а не +64 СВЕРХ лимита
                // (иначе чанк 576 > бюджета §6.4 и может > контекста эмбеддера).
                chunks.extend(self.sliding_window_with_inner_overlap(section));
            }
        }
        // token_count записываем по ФИНАЛЬНОМУ содержимому каждого чанка.
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
│  1. embed_query: префикс search_query + L2-норма      │
│                                                       │
│  2. Hybrid Search:                                    │
│     ┌─────────────────┐   ┌────────────────────┐     │
│     │ Vector Search   │   │  FTS5 Full-text    │     │
│     │ usearch HNSW    │   │  BM25 ranking      │     │
│     │ top-20 chunks   │   │  top-20 chunks     │     │
│     └────────┬────────┘   └─────────┬──────────┘     │
│              └──────────┬───────────┘                 │
│                         │                             │
│  3. RRF Fusion          ▼                             │
│     Reciprocal Rank Fusion → топ 10 чанков            │
│                                                       │
│  4. Graph rank — 3-й источник в RRF (НЕ +0.2):         │
│     близость в графе к открытому файлу ранжируется     │
│     наравне с vector/FTS, в шкале RRF (~1/60)          │
│                                                       │
│  5. Dedup смежных чанков по char_start/char_end        │
│     (overlap не дублируется в контексте)               │
│                                                       │
│  6. Re-rank (опц.): ОТДЕЛЬНЫЙ bge-reranker или         │
│     LLM-listwise, SLA ~0.5–1s (НЕ «<200ms»;            │
│     chat-модель Qwen3-27B reranking не делает)         │
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

Два режима, оба асинхронны и не блокируют редактор. **Режим 2 (LLM) — НЕ на каждый save** (С-8): по явному действию пользователя или idle-debounce 30–60с после окончания правок, с дедупом по хэшу контента. Очереди embed / chat / suggest разнесены, чтобы suggest не душил чат на единственном инстансе.

**Режим 1 — Embedding similarity (быстрый, ~100ms):**
```rust
// ai/suggest.rs
pub async fn suggest_by_similarity(&self, file_id: FileId) -> Result<Vec<LinkSuggestion>> {
    // НЕ «среднее по чанкам»: центроид многотемной заметки бесполезен, и колонки
    // файлового вектора в схеме нет. Берём max-sim по ЧАНКАМ.
    let chunks = self.reader.get_chunk_vectors(file_id).await?;
    let exclude = self.reader.get_linked_files(file_id).await?;
    let mut by_file: HashMap<FileId, f32> = HashMap::new();
    for v in &chunks {
        for hit in self.ann.search(v, 20, &exclude)? {        // usearch HNSW
            let e = by_file.entry(hit.file_id).or_insert(0.0);
            *e = e.max(hit.score);                            // агрегируем max по файлам
        }
    }
    // Порог калибруется per-model на eval-наборе (0.75 — заглушка, зависит от эмбеддера).
    Ok(top_by_score(by_file, threshold_for(self.embedder.model_id()), 5))
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

### 6.5 Версионирование эмбеддингов и переэмбеддизация при смене модели

Размерность и качество вектора зависят от модели эмбеддера. Метаданные хранятся в `settings`: `embedding.model`, `embedding.dim`, `embedding.version`. На старте индексатор сверяет их с активным `EmbeddingProvider`:

```
если settings.embedding.model != active.model_id()  ИЛИ
   settings.embedding.dim   != active.dim():
      → пересоздать usearch-индекс под новую размерность
      → пометить все файлы как требующие переэмбеддизации (indexed_at = 0)
      → фоновая полная переэмбеддизация с прогрессом
```

Без этого смена модели (напр. `nomic 768` → `bge-m3 1024`) тихо ломает семантический поиск — урок SA Agent (старые 1024-векторы авто-скипались при переходе на 768).

### 6.6 Eval-харнесс качества RAG (обязателен, Фаза 1)

«Correctness over speed» неисполним без измерения корректности. Заводим (по образцу `sa-eval` из SA Agent):

- **Golden-набор** `вопрос → ожидаемые чанки/файлы` по реальному vault.
- **Метрики**: recall@8, nDCG, MRR; для атрибуции источников — отдельный eval.
- **Регрессия** до/после каждого изменения чанкера, RRF-весов, порогов, модели.
- Правило «сравнение валидно только при совпадении условий» (модель, сервер, набор).

Решения «RRF top-20 / порог 0.75 / 8 чанков» калибруются на этом наборе, а не берутся с потолка.

### 6.7 Контекст: дедуп overlap

При сборке топ-8 чанков (§6.4) смежные чанки одного файла сливаются по `char_start/char_end` (overlap не дублируется), и бюджет §6.4 считается уже по уникальному тексту.

---

## 7. Плагинная система подробно

### 7.1 Архитектура

```
┌──────────────────────────────────────────────────────────────┐
│                  Plugin Host = capability broker             │
│  Доверенный JS-плагин → Worker (логика) + main-thread (CM6)   │
│  UI-вью → sandbox-iframe ↔ MessagePort ↔ broker (ADR-002)     │
│  Broker — РЕАЛЬНАЯ граница прав; sandbox iframe сам по себе    │
│  её НЕ обеспечивает (capability laundering — см. §11)          │
└──────────────────────────────────────────────────────────────┘

plugins/  (НЕ в git: ставится из реестра по id@version#sha256, ADR-002)
└── {plugin-id}/
    ├── manifest.json     # метаданные + min_api_version + scoped permissions + signature
    ├── main.js           # точка входа, JS/TS (ESM) — ADR-001
    ├── compute.wasm      # ОПЦ. тяжёлые вычисления, вызывается из main.js
    └── ui/               # ОПЦ. изолированный UI-вью (iframe)
        ├── index.html
        └── styles.css
```

### 7.2 Manifest плагина

```json
{
  "id": "nexus-ai-suggest",
  "name": "AI Link Suggestions",
  "version": "1.2.0",
  "min_api_version": "1.2",          // МИНИМУМ версии ядра (НЕ "^1.0": каретка = "любой 1.x",
                                     // и плагин под фичи v1.2 падал на ядре 1.0 — С-13)
  "max_api_version": "2.0",          // опционально
  "entry": "main.js",                // JS-first (ADR-001)
  "ui": "ui/index.html",             // опц. изолированный вью
  "dependencies": { "nexus-core-graph": ">=1.1" },   // межплагинные зависимости
  "permissions": {
    "vault:read":  ["Notes/**", "!Private/**"],      // path-scoped, НЕ весь vault (ADR-002)
    "vault:write": ["Notes/**"],
    "ai:embed":    true,
    "ai:complete": { "local_only": true },           // запрет тихой отправки в облако
    "net":         ["api.example.com"],              // сеть — явно (в v1.0 такого права не было)
    "ui":          ["sidebar-right", "status-bar"]
  },
  "settings_schema_version": 2,      // миграция настроек между версиями плагина
  "settings_schema": { "threshold": { "type": "number", "default": 0.75, "min": 0.5, "max": 1.0 } },
  "signature": "ed25519:…"           // обязательна для marketplace с Фазы 2 (ADR-002)
}
```

### 7.3 Plugin API (TypeScript SDK)

> Исполняется в доверенном JS-контексте (ADR-001): `registerEditorExtension` отдаёт живое
> CodeMirror-расширение в main-контекст; колбэки и `AsyncIterable` работают как обычный JS
> (в v1.0 это было невыразимо в WASM). Каждый вызов проходит через broker и проверку
> scoped-прав (§7.4; протокол и capability-токены — §7.9). `ai.complete` уважает `local_only`.
> Для UI-вью в iframe тот же API проксируется по `MessagePort` (§7.5), но редакторные
> расширения там недоступны (матрица §7.7).

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

### 7.4 Host broker и изоляция (Rust сторона)

```rust
// plugin/broker.rs — broker = РЕАЛЬНАЯ граница прав (ADR-002).
// Identity плагина — по выделенному порту/каналу, НЕ по pluginId из сообщения
// (иначе confused deputy: плагин A назовётся B и заберёт его права).
pub struct PluginBroker {
    sessions: HashMap<PortId, PluginSession>,   // порт → плагин: источник истины identity
    audit: AuditLog,                            // обязательный неотключаемый журнал доступа
}

impl PluginBroker {
    pub async fn handle(&mut self, port: PortId, req: ApiRequest) -> Result<ApiResponse> {
        let s = self.sessions.get(&port).ok_or(PluginError::UnknownSession)?; // identity по порту
        self.check_scoped_permission(s, &req)?;            // path-scoped, local_only, net-allowlist
        let path = resolve_vault_path(&s.vault_root, req.path())?;  // канонизация + запрет выхода
        self.audit.record(s.id, &req);                     // read/write, объём egress, AI-вызовы
        dispatch(s, req, path).await
    }
}

// Единая канонизация для ВСЕХ host-функций и Tauri-команд (анти-traversal):
fn resolve_vault_path(root: &Path, p: &Path) -> Result<PathBuf> {
    let full = root.join(p).canonicalize()?;               // резолвит .. и симлинки
    if !full.starts_with(root) { return Err(PluginError::PathEscape); } // блок ../../.ssh и пр.
    Ok(full)
}

// ОПЦ. WASM (compute.wasm) — НАСТОЯЩИЕ лимиты, не tokio::timeout (он не прерывает busy-loop):
//   Config::epoch_interruption + Store::set_epoch_deadline    // CPU-лимит 5s реально
//   StoreLimitsBuilder::memory_size(256 MB) + ResourceLimiter // лимит памяти
//   Config::async_support + call_async + func_wrap_async      // async host-функции
//   Engine::precompile_module → .cwasm, Module::deserialize   // холодный старт
```

### 7.5 UI-вью (iframe-изоляция через MessagePort)

```typescript
// UI-вью рендерится в sandbox-iframe. Хост при инициализации передаёт ОДИН MessagePort
// через transfer; дальше плагин общается ТОЛЬКО по своему порту. Identity определяется
// портом на стороне хоста — pluginId из payload НЕ доверяется (opaque origin "null"
// делает event.origin бесполезным, а pluginId подделываемым — С-2).

// Внутри iframe плагина (порт получен от хоста, не самоназначен):
const nexus = NexusPluginUI.fromPort(portFromHost);
await nexus.vault.readFile('Notes/example.md');   // проксируется по порту → broker (§7.4)

// Редакторные расширения (CodeMirror декорации/виджеты) ЗДЕСЬ недоступны — их нельзя
// передать через postMessage; они только у доверенных in-process JS-плагинов (см. §7.6).
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
- Command Palette (Cmd+P) — плагины добавляют команды (через общий command-registry, §4.x)
- Context Menu — правая кнопка в редакторе и файловом дереве
- CodeMirror Extensions — кастомные inline-виджеты (ТОЛЬКО in-process JS-плагины)
- Graph overlays — доп. визуализации поверх графа
```

### 7.7 Матрица «тип плагина × возможности»

| Возможность | JS in-process | UI-вью (iframe) | WASM (compute) |
|---|:---:|:---:|:---:|
| Редакторные расширения CM6 (декорации/виджеты/autocomplete) | ✅ | ❌ | ❌ |
| Команды, context-menu, status-bar | ✅ | ✅ | — |
| Свой UI-вью (панель) | ✅ | ✅ | — |
| `vault.*` / `ai.*` (через broker, scoped) | ✅ | ✅ (по порту) | ✅ (host-функции) |
| Прямой доступ к чужому DOM/CM6 | ❌ | ❌ | ❌ |
| Чистые тяжёлые вычисления | — | — | ✅ |

> Так никто не обещает inline-виджеты iframe-песочнице, которая их структурно не тянет (Б1).

### 7.8 Жизненный цикл плагина

`install → enable → update → (rollback) → disable → uninstall`. Ключевое:
- **Hot enable/disable** без рестарта (для JS/iframe; `restart_required` — только для native-core).
- **Update/rollback**: установка из реестра по `id@version#sha256`, проверка целостности; откат на предыдущую версию.
- **Миграция настроек** по `settings_schema_version` (хуки, как у миграций БД).
- **`onUninstall`**: очистка настроек, данных, подписок плагина. Изоляция данных между плагинами (namespace).
- **Несовместимость**: если версия ядра вне `[min_api_version, max_api_version]` — плагин не грузится, понятная ошибка (С-13).

### 7.9 Протокол broker ↔ плагин (wire-format, capability-токены, audit)

Транспорт: при загрузке хост создаёт на плагин один `MessageChannel` и передаёт плагину один `MessagePort` через `transfer`. Весь обмен — только по этому порту (**identity = порт**, не `pluginId` из payload).

**Конверт сообщения** (structured-clone, без функций/DOM):
```jsonc
// request
{ "id": "uuid", "cap": "<capability-токен сессии>", "method": "vault.readFile", "args": { "path": "Notes/x.md" } }
// response
{ "id": "uuid", "ok": true,  "result": "…" }
{ "id": "uuid", "ok": false, "error": { "code": "PathEscape|Denied|Timeout|…", "message": "…" } }
// стрим (ai.complete) — серия по тому же порту, коррелируется по id:
{ "id": "uuid", "kind": "token|usage|done|error", "text": "…" }
```

**Capability-токен:**
- выдаётся хостом при загрузке, привязан к `(pluginId, scoped-права, vault_root)`, случаен и неугадываем, хранится в сессии хоста;
- проверяется на КАЖДЫЙ вызов; `pluginId`/права из `args` — НЕ источник истины;
- ревокация (disable/uninstall/смена прав) инвалидирует токен немедленно.

**Путь проверки вызова** (broker, §7.4): `сессия по порту → cap валиден → method ∈ permissions → resolve_vault_path в scope → (ai:complete) local_only соблюдён / (net) host в allowlist → audit.record → dispatch`.

**Audit-log** (неотключаемый, на сессию), запись:
```jsonc
{ "ts": 0, "pluginId": "…", "method": "…", "path": "…?", "bytes_out": 0, "provider": "local|cloud", "decision": "allow|deny" }
```
Доступен пользователю; основа для расследования утечки и rate-limiting egress.

**Изоляция и устойчивость:** вызов плагина изолирован — trap WASM / reject JS возвращается как `{ ok:false, error }` и не роняет хост; на вызов (вне стрима) — таймаут; отмена — по `id`. **Версия протокола** (`protocol_version`) согласуется в handshake; несовместимость → отказ загрузки.

---

## 8. Git-sync протокол

### 8.1 Структура .gitignore

```gitignore
# Nexus: индексы и векторы (пересоздаются локально)
.nexus/nexus.db
.nexus/*.db-*
.nexus/vectors.usearch
.nexus/cache/

# Nexus: локальные/секьюрные настройки — НЕ в git (ADR-002: endpoints, remote, ключи)
.nexus/local.json

# Nexus: исполняемый код плагинов — НЕ в git (ADR-002: git-доставка кода = вектор атаки;
# синхронизируется только декларация id@version#sha256 в config.json + настройки)
.nexus/plugins/

# Конфликты merge — не индексируем как заметки
*.conflict

# Синхронизируем конфиг и темы (НЕ плагины)
!.nexus/config.json
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
    // git2 СИНХРОННЫЙ → весь метод выполняется в spawn_blocking/git-actor, НЕ на async-рантайме
    // (С-1: иначе add_all/commit/pull заморозят все IPC). Триггер — по idle (default 60с покоя)
    // + squash, а не безусловно каждые 300с.
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

        // НЕ add_all(["*"]) — он коммитит секреты/мусор и затирает ручной стейджинг (С-16).
        // Стейджим только изменённые vault-пути одобренных типов, после secret-scan.
        let mut index = self.repo.index()?;
        for path in &changed_files {
            if is_syncable(path) && !secret_scan_hit(path) {   // *.md, config.json, themes/**
                index.add_path(Path::new(path))?;
            }
        }
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

**Sync-lock и буфер редактора (С-17).** На время pull/merge watcher и индексация приостанавливаются (иначе pull → шторм реиндексации → коммит полусмерженного состояния). Если на диске изменился файл, открытый с грязным буфером CodeMirror, — показываем diff/выбор «диск vs ваши правки», не перетираем молча. `.conflict`-файлы исключены из FTS/графа.

### 8.4 Будущий мобильный клиент

```
[Desktop]  ←─git pull/push─→  [Remote repo]  ←─git pull/push─→  [Mobile]
    │                                                                  │
    │  .md файлы + config.json синхронизируются                       │
    │                                                                  │
    │  nexus.db НЕ синхронизируется →                                 │
    │  каждое устройство строит свой индекс локально                  │
    │                                                                  │
    │  Плагины (код) НЕ синхронизируются (ADR-002): едет только        │
    │  декларация id@version#sha256; mobile ставит/не ставит сам       │
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
    "indexing_one": "Indexing {{count}} file...",
    "indexing_other": "Indexing {{count}} files...",
    "indexed_one": "Vault indexed ({{count}} note)",
    "indexed_other": "Vault indexed ({{count}} notes)",
    "_ru_plural_note": "ru требует _one/_few/_many: «1 файл / 2 файла / 5 файлов» (С-11)",
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

// Детекция системной локали при ПЕРВОМ запуске (до чтения config): tauri-plugin-os /
// navigator.language — иначе новый русскоязычный пользователь сразу получит en (С-11).
```

---

### Что усилено в v1.1 (С-11 / С-12)

- **Плюрализация**: ключи `_one/_few/_many` + ICU-плюралы i18next для ru (иначе «5 файл»).
- **Форматирование**: числа/даты — через `Intl.NumberFormat` / `Intl.DateTimeFormat` с активной локалью (50000 → «50 000»).
- **Сортировка дерева/списков** — `Intl.Collator(locale, { numeric: true })` (кириллица, Ё, регистр), а не строковый `<`.
- **i18n бэкенда (Rust)** — `fluent`/`rust-i18n` с общим источником ключей; локаль прокидывается из фронта; ошибки индексатора/git/AI локализуются (не только en).
- **i18n плагинов** — SDK предоставляет i18n-API; ключи в namespace `plugin:<id>:<key>`; требование RU/EN для плагинного UI.
- **Кросс-язычный RAG** — следствие выбора эмбеддера: мультиязычная модель (bge-m3 / multilingual-e5), иначе запрос на ru по en-заметке даёт низкий similarity (ADR-005).
- CI-линтер недостающих ключей между ru/en.

---

## 10. Производительность

### Целевые метрики (vault 50k файлов)

| Операция | Целевое время | Как достигается |
|---|---|---|
| UI интерактивен (редактор) | < 1 секунды | Редактор виден сразу; индекс/граф/usearch-mmap — в фоне, НЕ на критическом пути (С-15) |
| Готовность индекса/графа | фон, не блокирует UI | Беклинки — из SQLite по индексу; графа в памяти НЕТ (ADR-004) |
| Открытие файла | < 50ms | Файл уже на диске, CodeMirror быстрый |
| Full-text поиск | < 100ms | FTS5 с оптимизированными индексами |
| Семантический поиск | < 300ms | usearch HNSW + префильтр по метаданным; sqlite-vec (flat-скан) для этого НЕ годится |
| RAG запрос (до первого токена) | < 1s | Параллельный поиск + быстрый промпт-билд |
| Индексирование одного файла | < 500ms | Параллельный embed + SQLite write |
| Полная (ре)индексация | бюджет в ЧАНКАХ, не «<30 мин» | 50k файлов ≈ 150–400k чанков; время = чанки / измеренный throughput эмбеддера; прогресс «N/M», резюмируемо после краха (С-15) |
| Граф: layout + рендер | layout в Web Worker | Узкое место — force-layout, не отрисовка; основной режим — локальный N-hop, глобальный опц. |
| Предложения связей | < 2s | Кэш в БД, пересчёт только при изменении |

### Ключевые оптимизации

**1. Граф — из SQLite, без in-memory копии (ADR-004):**
```rust
// Беклинки/обходы — запросами по индексу idx_links_target (page-cache, доли мс).
// petgraph в памяти НЕ держим: дублирование + рассинхрон трёх локов + hydrate в бюджете старта.
// Локальный граф для вью — рекурсивный CTE по links, ограниченный N-hop.
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

### Модель угроз (от кого защищаемся)

| Актор | Чем адресуется |
|---|---|
| Вредоносный плагин (community) | capability-broker + path-scoped права + audit-log + подпись |
| Вредоносное обновление плагина | подпись + хэш `id@version#sha256`; кода нет в git |
| Вредоносный синхронизированный/импортированный vault | плагины из чужого vault → `disabled, needs-review`; анти-prompt-injection |
| Скомпрометированный remote / коллаборатор | код не едет через git; verify remote (host-key); secret-scan коммитов |
| Локальный доступ к ФС (кража ноута) | опц. at-rest шифрование (SQLCipher), ключ в keychain |
| SSRF / утечка через конфиг | валидация `*.url`; секьюр-поля вне git |
| Prompt injection через заметки | разделители контекста, валидация JSON-ответов, пометка непроверенного |

### Плагинная безопасность (ADR-002)

```
Граница безопасности = HOST-SIDE CAPABILITY BROKER, а не iframe-sandbox.
(sandbox блокирует прямой fetch плагина, но НЕ проксированную способность: плагин с
 vault:read + ai:complete мог бы слить весь vault руками хоста — capability laundering.)

Уровень 1 — Identity и брокер:
  └── На плагин — отдельный MessagePort; identity по порту, НЕ по pluginId из payload (С-2)
  └── Каждый вызов API → check_scoped_permission + resolve_vault_path (канонизация, анти-traversal)
  └── Обязательный неотключаемый audit-log: что читал/писал, объём egress, AI-вызовы

Уровень 2 — Path-scoped permissions (вместо install-time «как Android»):
  └── vault:read/write — по glob-путям; секретные папки недоступны без отдельного гранта
  └── runtime-grant для чувствительного; отзыв в любой момент; consent при эскалации на update
  └── ai:complete { local_only } — запрет тихой отправки в облако; net — по allowlist хостов

Уровень 3 — Изоляция исполнения:
  └── JS-логика — Worker; UI-вью — sandbox-iframe + строгий CSP (без unsafe-inline/eval)
  └── Опц. WASM — epoch/fuel + StoreLimits (РЕАЛЬНЫЕ 5s/256MB, не tokio::timeout) — §7.4
  └── Tauri-команды vault.*/git.* НЕ доступны из iframe-контекста плагина — только через broker

Уровень 4 — Дистрибуция и подпись (с Фазы 2, НЕ v2.0):
  └── Реестр: id → верифицированный издатель; целостность по sha256; blocklist/kill-switch
  └── Код плагинов НЕ в git; приход через pull → disabled, needs-review (consent на устройстве)
```

### Данные и приватность

```
- Vault не уходит в облако без явного opt-in; cloud-fallback — только chat, с индикацией (С-2)
- Эндпоинты (*.url) валидируются (анти-SSRF): по умолчанию loopback; приватные/metadata-диапазоны
  блокируются; изменённый из pull base_url не применяется без подтверждения (С-18)
- API-ключи — в OS keychain; секьюр-поля (endpoints/remote) — в .nexus/local.json, не в git
- At-rest: опц. шифрование nexus.db (SQLCipher), ключ в keychain — прежде всего chat_messages/chunks/FTS
- Prompt injection: контент заметок в RAG/suggest обрамляется неподделываемыми разделителями
  (не XML <note>, который ломается контентом); JSON-ответы строго валидируются; контент из
  непроверенных источников помечается (С-19)
- Логи: тип Redacted<T> (контент и пути не печатаются в Debug) + redaction-layer; crash-reporter
  скрабит пути (~ / хэш), строго opt-in, предпросмотр payload
```

---

## 12. Фазы разработки

### Фаза 0 — Фундамент (4-6 недель)

**Цель:** Работающий редактор с правильной архитектурой, без AI.

**Deliverables:**
- [ ] Репозиторий: monorepo с pnpm workspaces + Cargo workspace
- [ ] Tauri 2 приложение запускается на Win/Mac/Linux
- [ ] Открытие vault-папки, ленивое файловое дерево (listDir, не 50k одним IPC)
- [ ] **Модель вкладок/групп (workspace)** — НЕ одиночный currentFile (ретрофит = переписывание ядра)
- [ ] CodeMirror 6: source-mode + подсветка + клик/автокомплит `[[wikilink]]`. **Live Preview — отдельный эпик, НЕ Фаза 0** (С-22)
- [ ] **Command registry** (ядро + база для плагинного registerCommand) + keymap
- [ ] `#tags` — парсинг, фильтрация
- [ ] SQLite via **rusqlite + write-actor** (ADR-003); раннер миграций; схема v1: files, links, tags
- [ ] Watcher: **notify-debouncer-full** + ignore-список + reconcile по пути (atomic-save сохраняет file_id)
- [ ] Беклинки — **из SQLite** по индексу (НЕ petgraph в памяти, ADR-004)
- [ ] Базовый граф-вью (**sigma.js**, layout в Web Worker — единый движок)
- [ ] Full-text поиск (FTS5 поверх chunks + триггеры)
- [ ] i18n RU/EN: плюралы `_one/_few/_many`, Intl-формат, Collator-сортировка, детекция системной локали
- [ ] CM6↔React контракт (view создаётся раз, dispatch на смену файла; guard StrictMode)
- [ ] Tauri capabilities/CSP-аудit (минимальный allowlist) — безопасность с Фазы 0

**Критерий успеха:** Можно открыть существующий Obsidian vault и работать с ним.

---

### Фаза 1 — AI Core (4-6 недель)

**Цель:** Полноценная AI интеграция с RAG.

**Deliverables:**
- [ ] **usearch (HNSW)** + таблица chunks; размерность вектора — ИЗ модели (не хардкод)
- [ ] Chunker: заголовки + sliding (**overlap внутри окна**) + токенайзер эмбеддера; frontmatter вырезан, code/таблицы атомарны
- [ ] Embedder: **отдельный embedding_url**, мультиязычная модель; `embed_query`/`embed_document` (префиксы) + **L2-нормализация**
- [ ] Эмбеддинг **по чанкам** (не по файлу!); batch + семафор к серверу; reconcile после краха
- [ ] Полная (ре)индексация с прогрессом «N/M чанков», резюмируемая; **переэмбеддизация при смене модели** (§6.5)
- [ ] Hybrid search: vector + FTS5 + RRF (**граф — 3-й ранг, без +0.2**) + dedup overlap
- [ ] Reranker (опц.): отдельный инстанс bge-reranker или LLM-listwise (НЕ «<200ms»)
- [ ] Чат-интерфейс; **streaming через per-session `Channel`** + финализация в историю + отмена (НЕ глобальный event)
- [ ] Контекст активной вкладки в чат
- [ ] Предложения связей — **max-sim по чанкам** (режим 1); режим 2 (LLM) — по idle/действию, не на save
- [ ] **Eval-харнесс** (golden Q→chunks, recall@8/nDCG) — калибровка порогов (§6.6)
- [ ] Cloud fallback — **только chat, opt-in**, с индикацией (настройка в UI)

**Критерий успеха:** Можно спросить "что я знаю о X?" и получить ответ с источниками.

---

### Фаза 2 — Plugin Ecosystem (3-4 недели)

**Цель:** Плагинная система готова к сторонним разработчикам.

**Deliverables:**
- [ ] **Capability broker** + MessagePort-identity + неотключаемый audit-log (граница безопасности, НЕ WASM-sandbox)
- [ ] **Path-scoped permissions** + runtime-grant + отзыв (вместо install-time «как Android»)
- [ ] Plugin API v1.0 (**JS-first SDK**): vault, ai, ui, settings; `min_api_version` в манифесте
- [ ] nexus-plugin-sdk npm: **реэкспорт CodeMirror 6 с жёстким пином** (запрет своего CM у плагина)
- [ ] CodeMirror extension API — для **доверенных in-process** JS-плагинов
- [ ] UI точки расширения: sidebar, status-bar, commands, context-menu (матрица §7.7)
- [ ] Опц. **WASM (epoch/fuel + StoreLimits)** для тяжёлых вычислений + canonical path-resolve
- [ ] **Подпись плагинов + реестр** (id→издатель, целостность sha256, blocklist) — ЗДЕСЬ, не v2.0
- [ ] Lifecycle: install/update/rollback/uninstall, миграция настроек, hot enable/disable
- [ ] Plugin marketplace UI (поиск, установка, обновление; код вне git)
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
- [ ] `nexus-git-sync` как **core module** (не sandbox-плагин): выборочный коммит + secret-scan, sync-lock, всё в spawn_blocking
- [ ] Conflict detection и UI; буфер vs диск (diff при грязном буфере)
- [ ] Умные commit-сообщения
- [ ] Нагрузочное тестирование: 50k файлов (как часть CI, а не разово)
- [ ] Оптимизация старта: UI < 1s, индекс/граф — фоном
- [ ] Граф: единый sigma.js + **layout в Web Worker** + локальный N-hop
- [ ] Файловое дерево: flatten-слой видимых узлов + ленивые дети + фильтр на стороне Rust
- [ ] Batch embedding оптимизация (throughput-бенч эмбеддера)
- [ ] Профилирование и устранение узких мест

**Критерий успеха:** Vault 50k файлов открывается за 3 секунды, поиск < 100ms.

---

### Фаза 4 — Polish + Mobile Prep (2-3 недели)

**Цель:** Production-ready десктоп, фундамент для мобилки.

**Deliverables:**
- [ ] Тема (светлая/тёмная + CSS-переменные) + протекание токенов темы в iframe-плагины
- [ ] Keyboard shortcuts (vim mode опц.) — поверх command-registry из Фазы 0
- [ ] Просмотр вложений/не-md: изображения, PDF, embeds `![[...]]`, Mermaid/LaTeX
- [ ] PDF / Print экспорт
- [ ] Опц. at-rest шифрование (SQLCipher), ключ в keychain
- [ ] Crash reporter: scrubbing путей (~/хэш), без контента, строго opt-in, предпросмотр
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
| ANN недостаточно быстр на 50k файлов | Средняя | Высокое | **Решено:** usearch (HNSW) с старта + префильтр (ADR/§3), не «benchmark потом» |
| llama.cpp API меняется | Низкая | Среднее | Раздельные Chat/Embedding-провайдеры изолируют изменения (ADR-005) |
| ~~WASM для плагинов слишком медленный~~ | — | — | **Снято:** JS-first основной рантайм (ADR-001); WASM — опц. для тяжёлых вычислений |
| **Live Preview уровня Obsidian — объём/сложность** | **Высокая** | **Высокое** | Отдельный эпик, не Фаза 0; в Фазе 0 — source-mode (С-22) |
| **Throughput эмбеддера на первом индексе** | Высокая | Среднее | Бюджет в чанках, мультихост, честный прогресс «N/M»; резюмируемость |
| **Кросс-язычный RAG (RU↔EN) слабый** | Средняя | Высокое | Мультиязычный эмбеддер (bge-m3/e5); проверяется на eval (ADR-005) |
| Конфликты git при активной мобильной работе | Высокая | Среднее | Three-way merge + UI + sync-lock + diff «буфер vs диск» |
| CodeMirror 6 плохо работает с 1MB+ файлами | Низкая | Высокое | Chunk rendering, только видимая область |
| ~~petgraph in-memory граф слишком большой~~ | — | — | **Снято:** источник истины графа — SQLite (ADR-004) |
| Экосистема плагинов не появится | Средняя | Высокое | **Реально митигируется** низким порогом входа: JS-SDK, `npm create`, hot-reload, типы, шаблоны (а не «first-party как демонстрация») |
| Утечка vault через плагин/облако | Средняя | Высокое | Capability-broker, path-scoped, `local_only`, audit-log, cloud opt-in (ADR-002) |

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

Плагин в манифесте указывает `min_api_version` (МИНИМУМ ядра), а НЕ `"^1.0"` — loader
блокирует несовместимое ДО загрузки (С-13). Runtime feature-detection — `api.capabilities`.

```
v1.0 → vault CRUD (scoped), ai.embed, ai.complete{local_only}, базовый UI, commands
v1.1 → graph read API, ai.searchSemantic
v1.2 → vault:watch, editor extensions (in-process JS)
v2.0 → опц. WASM-вычисления, graph write API, collaboration   // мажор = breaking
```

Политика breaking-changes: смена сигнатуры, сужение permission, формат settings. Deprecation —
минимум N миноров, `@deprecated` в типах, changelog/codemod. CodeMirror 6 пинится в SDK; его
апгрейд = breaking-событие плагинного API (иначе дублирование `@codemirror/state` роняет редактор).

---

*Документ живой. Обновляется по мере принятия архитектурных решений.*  
*Последнее обновление: 2026*
