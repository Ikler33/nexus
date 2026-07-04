/**
 * DTO-типы chat-домена (F-2b): сессии переписки, поиск по ней, память переписки, события
 * RAG-чат-стрима. Зеркала Rust-структур (`chat_log` / `commands::chat`) — контракт провода
 * `invoke`. Потребители импортируют их по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

// `SearchHit` (search-домен) и `EpisodeHit` (episode-домен) — payload'ы событий чат-стрима,
// принадлежат ЧУЖИМ доменам (вынесены в F-2d). Импорт type-only из их доменных источников —
// в рантайме стирается, цикла домен ↔ домен нет (тот же паттерн, что у lib/mock/*).
import type { EpisodeHit } from '../episode/types';
import type { SearchHit } from '../search/types';

/** Сессия чата (зеркалит Rust `chat_log::ChatSession`) — история-дропдаун AI-панели. */
export interface ChatSessionInfo {
  id: number;
  title: string;
  createdAt: number;
  updatedAt: number;
}

/** Совпадение поиска по переписке (#58, зеркалит Rust `chat_log::ChatSearchHit`). */
export interface ChatSearchHit {
  sessionId: number;
  title: string;
  role: 'user' | 'assistant';
  /** Фрагмент с подсветкой совпадений (FTS5 snippet, `[...]`). */
  snippet: string;
  createdAt: number;
  /** Саммари эпизода сессии (EP), если есть. */
  summary: string | null;
}

/** Сообщение сессии из БД (зеркалит `chat_log::StoredMessage`). */
export interface StoredChatMessage {
  role: 'user' | 'assistant';
  content: string;
  /** JSON-снапшот источников ({sources, webSources}) — как было показано. */
  sourcesJson: string | null;
  createdAt: number;
}

/** Типизированный отказ политики эгресса в стриме (AC-EGR-14): offline | feature | host; web — secret (W4). */
export type EgressDeniedKind = 'offline' | 'feature' | 'host' | 'secret' | 'notConfigured';

/** Web-источник (W-2): результат SearXNG-поиска — цитата web-ответа (зеркалит Rust `SearchResult`). */
export interface WebSource {
  title: string;
  url: string;
  snippet: string;
}

/** Фрагмент памяти переписки (N4b, зеркалит Rust `chat_log::MemoryHit`) — «из прошлых разговоров». */
export interface MemoryHit {
  sessionId: number;
  sessionTitle: string;
  role: string;
  snippet: string;
  score: number;
}

/**
 * Событие RAG-чат-стрима (зеркалит Rust `commands::chat::ChatStreamEvent`, тег `type`, camelCase).
 * Порядок: `sources` → (для reasoning-модели — живые `reasoningSummary`/`reasoning`) → много `token`
 * → `done` (или `error`). `reasoning` — сырой chain-of-thought (спойлер), `reasoningSummary` —
 * короткая живая сводка CoT («💭 …», R1); оба могут не приходить (non-reasoning модель).
 */
export type ChatStreamEvent =
  | { type: 'sources'; sources: SearchHit[] }
  | { type: 'webSources'; sources: WebSource[] }
  | { type: 'memorySources'; sources: MemoryHit[] }
  | { type: 'episodeSources'; sources: EpisodeHit[] }
  | { type: 'token'; text: string }
  | { type: 'reasoning'; text: string }
  | { type: 'reasoningSummary'; text: string }
  | { type: 'done'; full: string }
  | { type: 'error'; message: string; deniedKind?: EgressDeniedKind };
