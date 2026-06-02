import { create } from 'zustand';

import type { ChatStreamEvent, SearchHit } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';

/**
 * Состояние RAG-чата (Ф1-8). Сессия = лента сообщений в памяти. Стриминг ответа идёт через
 * `tauriApi.chat.streamRag` (Channel в Tauri, мок в браузере): `sources` → поток `token` → `done`.
 * Один активный стрим за раз (как бэкенд `AppState::begin_chat`); `stop` шлёт отмену.
 */

/** Источник ответа (RAG-чанк) — = `SearchHit`. */
export type ChatSource = SearchHit;

export interface ChatMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  /** Источники (для ответа ассистента) — приходят первым событием стрима. */
  sources?: ChatSource[];
  /** Идёт ли ещё стрим в это сообщение. */
  streaming?: boolean;
  /** Текст ошибки (retrieve/LLM), если стрим завершился неудачно. */
  error?: string;
}

interface ChatState {
  messages: ChatMessage[];
  streaming: boolean;
  /** Отправляет вопрос; `center` — путь открытого файла (граф-ранг в retrieval). */
  send: (question: string, center?: string) => void;
  /** Останавливает текущий стрим (если идёт). */
  stop: () => void;
  /** Очищает сессию (нельзя во время стрима — сначала `stop`). */
  clear: () => void;
}

let seq = 0;
const nextId = () => `m${++seq}`;

export const useChatStore = create<ChatState>((set, get) => {
  let cancelFn: (() => void) | null = null;

  /** Обновляет сообщение по id (иммутабельно). */
  const patch = (id: string, fn: (m: ChatMessage) => ChatMessage) =>
    set((s) => ({ messages: s.messages.map((m) => (m.id === id ? fn(m) : m)) }));

  return {
    messages: [],
    streaming: false,

    send(question, center) {
      const q = question.trim();
      if (!q || get().streaming) return;

      const userMsg: ChatMessage = { id: nextId(), role: 'user', content: q };
      const replyId = nextId();
      const reply: ChatMessage = { id: replyId, role: 'assistant', content: '', streaming: true };
      set((s) => ({ messages: [...s.messages, userMsg, reply], streaming: true }));

      const onEvent = (event: ChatStreamEvent) => {
        switch (event.type) {
          case 'sources':
            patch(replyId, (m) => ({ ...m, sources: event.sources }));
            break;
          case 'token':
            patch(replyId, (m) => ({ ...m, content: m.content + event.text }));
            break;
          case 'done':
            patch(replyId, (m) => ({ ...m, content: event.full || m.content, streaming: false }));
            cancelFn = null;
            set({ streaming: false });
            break;
          case 'error':
            patch(replyId, (m) => ({ ...m, error: event.message, streaming: false }));
            cancelFn = null;
            set({ streaming: false });
            break;
        }
      };

      cancelFn = tauriApi.chat.streamRag(q, onEvent, { center });
    },

    stop() {
      cancelFn?.();
      cancelFn = null;
      set((s) => ({
        streaming: false,
        messages: s.messages.map((m) => (m.streaming ? { ...m, streaming: false } : m)),
      }));
    },

    clear() {
      if (get().streaming) return;
      set({ messages: [] });
    },
  };
});
