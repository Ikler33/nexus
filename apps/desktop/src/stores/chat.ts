import { create } from 'zustand';

import { logUi } from '../lib/debug-log';
import { usePrefsStore } from './prefs';

import type {
  ChatStreamEvent,
  EgressDeniedKind,
  MemoryHit,
  SearchHit,
  WebSource,
} from '../lib/tauri-api';
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
  /** Память переписки (N4b): фрагменты прошлых диалогов, подмешанные в контекст ответа. */
  memorySources?: MemoryHit[];
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
  /** Закреплённые заметки (P6-PIN): их ПОЛНОЕ содержимое гарантированно идёт в контекст ИИ —
   *  «обсудить эту заметку» (не зависит от RAG-ретрива). Пути относительно vault, кап PIN_MAX. */
  pinned: string[];
  /** Закрепить/открепить заметку по пути (no-op во время стрима; кап PIN_MAX при добавлении). */
  togglePin: (path: string) => void;
  /** Снять все закрепления. */
  clearPins: () => void;
  /** CURATE: открепить пути под удалённым (delete файла/каталога) — не держим мёртвый пин. */
  dropPinsUnder: (path: string) => void;
  /** CURATE: переписать закреплённые пути при rename/move (своп префикса) — иначе после
   *  переименования на старый путь может лечь чужая заметка → неверный контекст ИИ. */
  renamePins: (from: string, to: string) => void;
  /** Отправляет вопрос; `center` — путь открытого файла (граф-ранг в retrieval, только в vault-режиме). */
  send: (question: string, center?: string) => void;
  /** Останавливает текущий стрим (если идёт). */
  stop: () => void;
  /** Очищает сессию (нельзя во время стрима — сначала `stop`). */
  clear: () => void;
  /** Текущая сессия в БД (`null` — ещё не создана; создастся первым завершённым обменом). */
  sessionId: number | null;
  /** Загружает сессию из БД в ленту (клик в истории). */
  loadSession: (id: number) => Promise<void>;
  /** Новая сессия: чистая лента, следующий обмен создаст запись в БД. */
  newSession: () => void;
  /**
   * Загружает сохранённую историю чата для vault (`root`) из localStorage; `null` (vault закрыт) —
   * очистка. Вызывается из `App.tsx` при смене корня vault. Персист идёт автоматически на терминальных
   * событиях (done/error/stop/clear).
   */
  hydrate: (root: string | null) => void;
}

let seq = 0;
const nextId = () => `m${++seq}`;

/** Максимум закреплённых заметок (P6-PIN) — бюджет контекста; бэкенд тоже капит. */
const PIN_MAX = 5;

export const useChatStore = create<ChatState>((set, get) => {
  let cancelFn: (() => void) | null = null;
  // Открыт ли vault (ставит hydrate) — без него обмены в БД не пишем.
  let vaultOpen = false;

  // Персист обмена в vault-БД (решение владельца 2026-06-12: переписка — часть «второго мозга»,
  // localStorage-история v1 заменена таблицами chat_sessions/chat_messages). Вызывается на
  // терминальном done: последний (вопрос, ответ) + JSON источников. Best-effort.
  const save = () => {
    if (!vaultOpen) return;
    const msgs = get().messages;
    const reply = msgs[msgs.length - 1];
    const ask = msgs[msgs.length - 2];
    if (!reply || reply.role !== 'assistant' || !ask || ask.role !== 'user') return;
    if (reply.error) return; // ошибочные обмены не персистим (нечего вспоминать)
    const sourcesJson =
      reply.sources?.length || reply.webSources?.length || reply.memorySources?.length
        ? JSON.stringify({
            sources: reply.sources ?? [],
            webSources: reply.webSources ?? [],
            memorySources: reply.memorySources ?? [],
          })
        : null;
    void tauriApi.chat.sessions
      .logExchange(get().sessionId, ask.content, reply.content, sourcesJson)
      .then((sid) => set({ sessionId: sid }))
      .catch(() => {});
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
    pinned: [],
    sessionId: null,

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
    togglePin(path) {
      if (get().streaming || !path) return; // во время стрима заморожено
      const has = get().pinned.includes(path);
      const pinned = has
        ? get().pinned.filter((p) => p !== path)
        : [...get().pinned, path].slice(-PIN_MAX); // кап: при переполнении вытесняем старейший
      logUi('chat:pin-toggle', `${has ? 'unpin' : 'pin'} (${pinned.length})`);
      set({ pinned });
    },
    clearPins() {
      if (get().streaming) return;
      set({ pinned: [] });
    },
    dropPinsUnder(path) {
      const under = (p: string) => p === path || p.startsWith(`${path}/`);
      const pinned = get().pinned.filter((p) => !under(p));
      if (pinned.length !== get().pinned.length) set({ pinned });
    },
    renamePins(from, to) {
      const map = (p: string) =>
        p === from ? to : p.startsWith(`${from}/`) ? `${to}${p.slice(from.length)}` : p;
      const cur = get().pinned;
      const remapped = cur.map(map);
      // Дедуп: rename на уже-закреплённый путь не должен плодить дубль.
      const pinned = remapped.filter((p, i) => remapped.indexOf(p) === i);
      if (pinned.some((p, i) => p !== cur[i]) || pinned.length !== cur.length) set({ pinned });
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
          case 'memorySources':
            // N4b: фрагменты прошлых диалогов — отдельная плашка «из прошлых разговоров».
            patch(replyId, (m) => ({ ...m, memorySources: event.sources }));
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
      const pinned = get().pinned;
      logUi('chat:send', `mode=${mode} web=${web} pins=${pinned.length} len=${question.length}`);
      cancelFn = tauriApi.chat.streamRag(q, onEvent, {
        center,
        grounded: mode === 'vault',
        web,
        rerank: usePrefsStore.getState().aiRerank,
        // N4b: память переписки (отдельный канал chat_vectors). Текущую сессию исключаем на бэке
        // по sessionId — не пересказываем ассистенту его же реплики из этого диалога.
        memory: usePrefsStore.getState().aiChatMemory,
        sessionId: get().sessionId,
        // P6-PIN: гарантированный контекст закреплённых заметок (полное содержимое).
        pinned: pinned.length ? pinned : undefined,
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
      // контекста — хвост финализируется в историю СТАРОГО vault, отмена уходит на бэкенд.
      if (get().streaming) get().stop();
      vaultOpen = root != null;
      // pinned ЧИСТИМ при смене vault: пути относительны хранилищу — иначе кросс-vault утечка
      // содержимого в контекст ИИ (одноимённый файл в новом vault) или мёртвые чипы.
      set({ messages: [], sessionId: null, pinned: [] });
      if (!vaultOpen) return;
      // Продолжаем последнюю сессию (поведение прежнего localStorage-хвоста, теперь из БД).
      void tauriApi.chat.sessions
        .list()
        .then((sessions) => {
          const last = sessions[0];
          if (last) void get().loadSession(last.id);
        })
        .catch(() => {});
    },

    async loadSession(id) {
      if (get().streaming) return; // во время стрима не прыгаем по истории
      try {
        const stored = await tauriApi.chat.sessions.messages(id);
        disclosureOpen.clear();
        const restored: ChatMessage[] = stored.map((m) => {
          let sources: ChatSource[] | undefined;
          let webSources: WebSource[] | undefined;
          let memorySources: MemoryHit[] | undefined;
          if (m.sourcesJson) {
            try {
              const parsed = JSON.parse(m.sourcesJson) as {
                sources?: ChatSource[];
                webSources?: WebSource[];
                memorySources?: MemoryHit[];
              };
              sources = parsed.sources?.length ? parsed.sources : undefined;
              webSources = parsed.webSources?.length ? parsed.webSources : undefined;
              memorySources = parsed.memorySources?.length ? parsed.memorySources : undefined;
            } catch {
              /* битый снапшот источников — сообщение без карточек */
            }
          }
          return {
            id: nextId(),
            role: m.role,
            content: m.content,
            sources,
            webSources,
            memorySources,
          };
        });
        set({ messages: restored, sessionId: id });
      } catch {
        /* сессия недоступна — лента не трогается */
      }
    },

    newSession() {
      if (get().streaming) return;
      logUi('chat:new-session');
      disclosureOpen.clear();
      set({ messages: [], sessionId: null });
    },
  };
});
