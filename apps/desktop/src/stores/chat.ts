import { create } from 'zustand';

import { logUi } from '../lib/debug-log';
import { usePrefsStore } from './prefs';

import type { ChatStreamEvent, EgressDeniedKind, SearchHit, WebSource } from '../lib/tauri-api';
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
  /** Типизированный отказ эгресса (AC-EGR-14) — рендерится i18n-баннером, не сырой строкой. */
  deniedKind?: EgressDeniedKind;
  /**
   * Живая короткая сводка размышления reasoning-модели (R1) — стримится в индикатор «думает».
   * Эфемерна (показывается только во время стрима, НЕ персистится). Сырой CoT (`reasoning`-событие)
   * сознательно не храним и не рендерим — только сводку.
   */
  reasoningSummary?: string;
  /** Web-источники (W-3): результаты SearXNG для web-режима — цитаты с URL. */
  webSources?: WebSource[];
}

/** Раскрытость аккордеонов источников ВНЕ React-состояния (см. ChatView.Disclosure): живёт со
 *  стором, чтобы чиститься вместе с историей (clear/hydrate) и в тестах. Не персистится. */
export const disclosureOpen = new Map<string, boolean>();

/** Режим чата: по vault (RAG) / общий. Web — НЕ режим, а дополнительный флаг (`web`): «модель
 *  может сходить в интернет за уточнениями» поверх любого режима (ревизия владельца 11.06). */
export type ChatMode = 'vault' | 'general';

interface ChatState {
  messages: ChatMessage[];
  streaming: boolean;
  /** Режим чата: `vault` (RAG по заметкам) или `general` (общий, без грунтинга). */
  mode: ChatMode;
  /** Web-флаг ПОВЕРХ режима: разрешить модели интернет-поиск (web-агент решает сам, нужен ли). */
  web: boolean;
  /** Переключает режим (нельзя во время стрима). */
  setMode: (mode: ChatMode) => void;
  /** Тоггл web-флага (нельзя во время стрима). Режим не трогает. */
  toggleWeb: () => void;
  /** Отправляет вопрос; `center` — путь открытого файла (граф-ранг в retrieval, только в vault-режиме). */
  send: (question: string, center?: string) => void;
  /** Останавливает текущий стрим (если идёт). */
  stop: () => void;
  /** Очищает сессию (нельзя во время стрима — сначала `stop`). */
  clear: () => void;
  /**
   * Загружает сохранённую историю чата для vault (`root`) из localStorage; `null` (vault закрыт) —
   * очистка. Вызывается из `App.tsx` при смене корня vault. Персист идёт автоматически на терминальных
   * событиях (done/error/stop/clear).
   */
  hydrate: (root: string | null) => void;
}

let seq = 0;
const nextId = () => `m${++seq}`;

/** Префикс ключа localStorage для истории чата (на каждый vault — свой). */
const CHAT_KEY_PREFIX = 'nexus.chat.v1:';
/** Максимум сохраняемых сообщений (хвост) — защита localStorage от разрастания (см. docs/BACKLOG). */
const MAX_PERSISTED = 100;

export const useChatStore = create<ChatState>((set, get) => {
  let cancelFn: (() => void) | null = null;
  // Ключ localStorage текущего vault (ставит `hydrate`); `null` — vault не открыт, не персистим.
  let vaultKey: string | null = null;

  // Сохраняет историю текущего vault (хвост ≤MAX_PERSISTED, без стрим-флагов). Вызывается на
  // терминальных событиях. Best-effort: localStorage может быть недоступен/переполнен.
  const save = () => {
    if (!vaultKey) return;
    try {
      const msgs = get()
        .messages.slice(-MAX_PERSISTED)
        // Живую сводку не персистим — она эфемерна (показывается только в фазе «думает»).
        .map((m) => ({ ...m, streaming: false, reasoningSummary: undefined }));
      localStorage.setItem(vaultKey, JSON.stringify(msgs));
    } catch {
      /* недоступно/переполнено — не критично */
    }
  };

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
    mode: 'vault',
    web: false,

    setMode(mode) {
      if (get().streaming) return; // не переключаем режим на лету
      set({ mode });
    },
    toggleWeb() {
      if (get().streaming) return; // во время стрима флаг заморожен (как режим)
      const web = !get().web;
      logUi('chat:web-toggle', web ? 'on' : 'off');
      set({ web });
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

      // Применяет накопленный буфер токенов одним апдейтом (вызывается из rAF).
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
          case 'webSources':
            // W-3: цитаты web-агента (title/url/snippet) — рендерятся со ссылками наружу.
            patch(replyId, (m) => ({ ...m, webSources: event.sources }));
            break;
          case 'token':
            // Не set() на каждый токен — копим в буфер, рендерим раз в кадр (AC-Б10-4).
            pending += event.text;
            scheduleFlush();
            break;
          case 'reasoning':
            // Сырой chain-of-thought сознательно НЕ рендерим (решение владельца): в UI идёт только
            // живая сводка (`reasoningSummary`). Событие принимаем и игнорируем.
            break;
          case 'reasoningSummary':
            // Живая короткая сводка — стримится в индикатор «думает». Редкое событие (~1.5с),
            // патчим напрямую (без буфера).
            patch(replyId, (m) => ({ ...m, reasoningSummary: event.text }));
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
            save();
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
              deniedKind: event.deniedKind,
              streaming: false,
            }));
            cancelFn = null;
            set({ streaming: false });
            save();
            break;
          }
        }
      };

      const mode = get().mode;
      const web = get().web;
      logUi('chat:send', `mode=${mode} web=${web} len=${question.length}`);
      cancelFn = tauriApi.chat.streamRag(q, onEvent, {
        center,
        grounded: mode === 'vault',
        web,
        rerank: usePrefsStore.getState().aiRerank,
      });
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
      save();
    },

    clear() {
      disclosureOpen.clear();
      if (get().streaming) return;
      set({ messages: [] });
      save();
    },

    hydrate(root) {
      disclosureOpen.clear();
      // Смена vault при активном стриме (аудит 2026-06-10): дорезаем осиротевший стрим ДО смены
      // ключа — хвост финализируется в историю СТАРОГО vault (не утечёт в новый), отмена уходит
      // на бэкенд (LLM не молотит по закрытому vault).
      if (get().streaming) get().stop();
      vaultKey = root ? CHAT_KEY_PREFIX + root : null;
      if (!vaultKey) {
        set({ messages: [] });
        return;
      }
      let restored: ChatMessage[] = [];
      try {
        const raw = localStorage.getItem(vaultKey);
        if (raw) {
          const parsed: unknown = JSON.parse(raw);
          if (Array.isArray(parsed)) {
            restored = (parsed as ChatMessage[]).map((m) => ({ ...m, streaming: false }));
          }
        }
      } catch {
        /* битый JSON / нет localStorage — пустая история */
      }
      set({ messages: restored });
    },
  };
});
