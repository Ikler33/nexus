import type { ChatSearchHit, ChatSessionInfo, StoredChatMessage } from '../tauri-api';

/** In-memory мок сессий чата (превью/тесты вне Tauri): живая история без бэкенда. */
let nextId = 3;
const sessions: ChatSessionInfo[] = [
  { id: 1, title: 'Гибридный поиск и RRF', createdAt: 1781100000, updatedAt: 1781170000 },
  { id: 2, title: 'Выбор GPU для LLM-рига', createdAt: 1781000000, updatedAt: 1781050000 },
];
const messages = new Map<number, StoredChatMessage[]>([
  [
    1,
    [
      { role: 'user', content: 'Как работает гибридный поиск?', sourcesJson: null, createdAt: 1781170000 },
      {
        role: 'assistant',
        content: 'FTS ловит точные термины, вектор — смысл; RRF сливает ранги.',
        sourcesJson: null,
        createdAt: 1781170001,
      },
    ],
  ],
  [2, []],
]);

export async function list(): Promise<ChatSessionInfo[]> {
  return [...sessions].sort((a, b) => b.updatedAt - a.updatedAt);
}

export async function messages_(id: number): Promise<StoredChatMessage[]> {
  return messages.get(id) ?? [];
}
export { messages_ as messages };

/**
 * #58 session-search мок: подстроковый поиск по телам сообщений (зеркалит FTS5-поиск бэкенда —
 * совпавшие сообщения со snippet-фрагментом, заголовком сессии). Пустой запрос → пусто.
 */
export async function search(query: string, limit = 50): Promise<ChatSearchHit[]> {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const hits: ChatSearchHit[] = [];
  for (const s of sessions) {
    for (const m of messages.get(s.id) ?? []) {
      const idx = m.content.toLowerCase().indexOf(q);
      if (idx < 0) continue;
      const start = Math.max(0, idx - 20);
      const snippet =
        (start > 0 ? '…' : '') +
        m.content.slice(start, idx) +
        `[${m.content.slice(idx, idx + query.length)}]` +
        m.content.slice(idx + query.length, idx + query.length + 30);
      hits.push({
        sessionId: s.id,
        title: s.title,
        role: m.role,
        snippet,
        createdAt: m.createdAt,
        summary: null,
      });
      if (hits.length >= limit) return hits;
    }
  }
  return hits;
}

export async function logExchange(
  sessionId: number | null,
  question: string,
  answer: string,
  sourcesJson: string | null,
): Promise<number> {
  const now = Math.floor(Date.now() / 1000);
  let id = sessionId ?? 0;
  if (!sessionId || !sessions.some((s) => s.id === sessionId)) {
    id = nextId++;
    sessions.push({ id, title: question.slice(0, 48), createdAt: now, updatedAt: now });
    messages.set(id, []);
  }
  const list = messages.get(id)!;
  list.push({ role: 'user', content: question, sourcesJson: null, createdAt: now });
  list.push({ role: 'assistant', content: answer, sourcesJson, createdAt: now });
  const s = sessions.find((s) => s.id === id)!;
  s.updatedAt = now;
  return id;
}

/** P6-RGN: удалить последний обмен сессии (user+assistant) — для регенерации ответа. */
export async function deleteLastExchange(sessionId: number | null): Promise<void> {
  if (sessionId == null) return;
  const list = messages.get(sessionId);
  if (!list || list.length < 2) return;
  if (list[list.length - 1].role === 'assistant' && list[list.length - 2].role === 'user') {
    list.splice(list.length - 2, 2);
  }
}

/** Тест-хук: полная очистка мока (сиды превью убираются). */
export function __reset(): void {
  sessions.length = 0;
  messages.clear();
  nextId = 1;
}
