import { Channel, invoke } from '@tauri-apps/api/core';
import * as mockSessions from '../../mock/sessions';
import * as mockVault from '../../mock/vault';
import { bridge, isTauri } from '../bridge';
import { classifyChatInvokeError } from './classifyError';
import type { ChatSearchHit, ChatSessionInfo, ChatStreamEvent, StoredChatMessage } from './types';

/**
 * Chat-домен (F-2b): RAG-чат-стрим + сессии переписки («второй мозг»: история, поиск, запись
 * обмена, экспорт в заметку). Request/response-вызовы — через `bridge` (Tauri ↔ мок `lib/mock/*`);
 * потребители ходят сюда по-прежнему через `tauriApi.chat` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */

export const chat = {
  /**
   * RAG-чат со стримингом (Ф1-7): события приходят в `onEvent` (`sources` → `token`… → `done`).
   * Возвращает функцию отмены текущего стрима. Вне Tauri — мок.
   *
   * Честное bridge-исключение (см. шапку `../bridge.ts`): стрим-команда с `Channel` — создаёт
   * канал, подвешивает `onmessage`, возвращает функцию отмены (отдельная команда `chat_cancel`) —
   * это не request/response-форма `bridge`, поэтому остаётся прямым `invoke`.
   */
  streamRag: (
    question: string,
    onEvent: (event: ChatStreamEvent) => void,
    opts?: {
      k?: number;
      center?: string;
      grounded?: boolean;
      web?: boolean;
      rerank?: boolean;
      memory?: boolean;
      /** MEM (AC-MEM-5): подмешивать сохранённые явные факты (память агента). ВЫКЛ по умолчанию. */
      agentMemory?: boolean;
      /** EP-2: подмешивать саммари прошлых сессий (эпизодическая память). ВЫКЛ по умолчанию. */
      episodic?: boolean;
      /** Reasoning-режим: «Глубокий» (CoT gemma, медленнее) vs «Быстрый». ВЫКЛ по умолчанию = Быстрый. */
      deep?: boolean;
      sessionId?: number | null;
      /** P6-PIN: пути закреплённых заметок — их полное содержимое в гарантированный контекст. */
      pinned?: string[];
    },
  ): (() => void) => {
    // P0-2 (mock-must-match-backend): мок получает ВСЕ опции команды `chat_rag` — раньше
    // rerank/memory/agentMemory/episodic/deep/pinned/sessionId/center молча выбрасывались,
    // и превью/тесты «зеленели» на усечённом контракте.
    if (!isTauri())
      return mockVault.streamChat(question, onEvent, {
        k: opts?.k,
        center: opts?.center,
        grounded: opts?.grounded,
        web: opts?.web,
        rerank: opts?.rerank,
        memory: opts?.memory,
        agentMemory: opts?.agentMemory,
        episodic: opts?.episodic,
        deep: opts?.deep,
        sessionId: opts?.sessionId,
        pinned: opts?.pinned,
      });
    const channel = new Channel<ChatStreamEvent>();
    channel.onmessage = onEvent;
    invoke<void>('chat_rag', {
      question,
      k: opts?.k,
      center: opts?.center,
      grounded: opts?.grounded,
      web: opts?.web,
      rerank: opts?.rerank,
      memory: opts?.memory,
      agentMemory: opts?.agentMemory,
      episodic: opts?.episodic,
      deep: opts?.deep,
      sessionId: opts?.sessionId,
      pinned: opts?.pinned,
      channel,
    }).catch((e: unknown) => {
      // U5: pre-stream Err (нет ai.chat) → typed banner, не вечный «думает» / сырая строка.
      const { message, deniedKind } = classifyChatInvokeError(e);
      onEvent({ type: 'error', message, deniedKind });
    });
    return () => {
      void invoke<void>('chat_cancel');
    };
  },

  /** Сессии чата («второй мозг» переписки): история, загрузка, запись обмена, экспорт. */
  sessions: {
    list: (): Promise<ChatSessionInfo[]> =>
      bridge<ChatSessionInfo[]>('chat_sessions_list', undefined, () => mockSessions.list()),

    /** #58 session-search: полнотекстовый поиск по переписке (snippet-подсветка + заголовок/саммари). */
    search: (query: string, limit?: number): Promise<ChatSearchHit[]> =>
      bridge<ChatSearchHit[]>('chat_search', { query, limit }, () =>
        mockSessions.search(query, limit),
      ),

    /** История сообщений сессии (для загрузки переписки в ленту). */
    messages: (id: number): Promise<StoredChatMessage[]> =>
      bridge<StoredChatMessage[]>('chat_session_messages', { id }, () =>
        mockSessions.messages(id),
      ),

    /** Запись обмена «вопрос+ответ» в сессию (`sessionId=null` → новая); возвращает id сессии. */
    logExchange: (
      sessionId: number | null,
      question: string,
      answer: string,
      sourcesJson: string | null,
    ): Promise<number> =>
      bridge<number>('chat_log_exchange', { sessionId, question, answer, sourcesJson }, () =>
        mockSessions.logExchange(sessionId, question, answer, sourcesJson),
      ),

    /** P6-RGN: удалить последний обмен сессии (перед регенерацией ответа) — чтобы не двоить историю. */
    deleteLastExchange: (sessionId: number | null): Promise<void> =>
      bridge<void>('chat_delete_last_exchange', { sessionId }, () =>
        mockSessions.deleteLastExchange(sessionId),
      ),

    /** «Сохранить в заметки» → относительный путь созданной заметки. */
    toNote: (id: number): Promise<string> =>
      bridge<string>('chat_session_to_note', { id }, () => mockSessions.toNote()),
  },
};
