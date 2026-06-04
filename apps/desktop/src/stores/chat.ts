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
  /** Режим: `true` — ответ по vault (RAG-ретрив + источники); `false` — общий чат без грунтинга (V4.4). */
  grounded: boolean;
  /** Переключает режим vault/общий (нельзя во время стрима). */
  setGrounded: (grounded: boolean) => void;
  /** Отправляет вопрос; `center` — путь открытого файла (граф-ранг в retrieval, только в vault-режиме). */
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

  // Троттлинг рендера токенов (AC-Б10-4 / ревью C9): копим текст в буфер и применяем одним set()
  // на кадр (requestAnimationFrame) — ≤~60 ре-рендеров/сек вместо O(токенов). Один стрим за раз.
  let pending = '';
  let rafId: number | null = null;
  const cancelFlush = () => {
    if (rafId != null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
  };

  /** Обновляет сообщение по id (иммутабельно). */
  const patch = (id: string, fn: (m: ChatMessage) => ChatMessage) =>
    set((s) => ({ messages: s.messages.map((m) => (m.id === id ? fn(m) : m)) }));

  return {
    messages: [],
    streaming: false,
    grounded: true,

    setGrounded(grounded) {
      if (get().streaming) return; // не переключаем режим на лету
      set({ grounded });
    },

    send(question, center) {
      const q = question.trim();
      if (!q || get().streaming) return;

      const userMsg: ChatMessage = { id: nextId(), role: 'user', content: q };
      const replyId = nextId();
      const reply: ChatMessage = { id: replyId, role: 'assistant', content: '', streaming: true };
      pending = '';
      cancelFlush();
      set((s) => ({ messages: [...s.messages, userMsg, reply], streaming: true }));

      // Применяет накопленный буфер одним апдейтом (вызывается из rAF).
      const flush = () => {
        rafId = null;
        if (!pending) return;
        const chunk = pending;
        pending = '';
        patch(replyId, (m) => ({ ...m, content: m.content + chunk }));
      };
      const scheduleFlush = () => {
        if (rafId == null) rafId = requestAnimationFrame(flush);
      };

      const onEvent = (event: ChatStreamEvent) => {
        switch (event.type) {
          case 'sources':
            patch(replyId, (m) => ({ ...m, sources: event.sources }));
            break;
          case 'token':
            // Не set() на каждый токен — копим в буфер, рендерим раз в кадр (AC-Б10-4).
            pending += event.text;
            scheduleFlush();
            break;
          case 'done': {
            cancelFlush();
            const tail = pending;
            pending = '';
            patch(replyId, (m) => ({
              ...m,
              content: event.full || m.content + tail,
              streaming: false,
            }));
            cancelFn = null;
            set({ streaming: false });
            break;
          }
          case 'error': {
            cancelFlush();
            const tail = pending;
            pending = '';
            patch(replyId, (m) => ({
              ...m,
              content: m.content + tail,
              error: event.message,
              streaming: false,
            }));
            cancelFn = null;
            set({ streaming: false });
            break;
          }
        }
      };

      cancelFn = tauriApi.chat.streamRag(q, onEvent, { center, grounded: get().grounded });
    },

    stop() {
      cancelFn?.();
      cancelFn = null;
      cancelFlush();
      const tail = pending;
      pending = '';
      set((s) => ({
        streaming: false,
        messages: s.messages.map((m) =>
          m.streaming ? { ...m, content: m.content + tail, streaming: false } : m,
        ),
      }));
    },

    clear() {
      if (get().streaming) return;
      set({ messages: [] });
    },
  };
});
