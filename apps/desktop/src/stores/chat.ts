import { create } from 'zustand';

import { logUi } from '../lib/debug-log';
import { isExplicitSave, stripSaveCommand } from '../lib/memory-intent';
import { useMemoryStore } from './memory';
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

/** MEM-5: исход сразу-сохранения факта. `saved` — реально создан (с id для отмены); `duplicate` — уже
 *  был; `error` — запись упала; `nothing` — извлекать нечего. ChatView маппит на тост. */
export type SaveResult =
  | { status: 'saved'; id: number; text: string }
  | { status: 'duplicate'; text: string }
  | { status: 'error' }
  | { status: 'nothing' };

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
 *  стором, чтобы чиститься вместе с историей (clear/hydrate) и в тестах. Не персистится.
 *  LRU-кап (audit B12): за очень длинную сессию ключей-сообщений накапливались бы тысячи; раньше при
 *  >500 ChatView делал полный `.clear()` — резко схлопывал ВСЕ раскрытые CoT-аккордеоны. Теперь
 *  вытесняем по одному старейшему, текущие раскрытия не страдают. API совместим с Map (get/set/size/clear). */
class DisclosureMap {
  private map = new Map<string, boolean>();
  private readonly maxSize = 600;

  get(key: string): boolean | undefined {
    return this.map.get(key);
  }

  set(key: string, value: boolean): void {
    this.map.delete(key); // переустановка двигает ключ в конец (свежесть для эвикции по порядку вставки)
    this.map.set(key, value);
    if (this.map.size > this.maxSize) {
      const oldest = this.map.keys().next().value;
      if (oldest !== undefined) this.map.delete(oldest);
    }
  }

  get size(): number {
    return this.map.size;
  }

  clear(): void {
    this.map.clear();
  }
}

export const disclosureOpen = new DisclosureMap();

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
  /** AIP-3: текст-предзаполнение композера (мост «Разобрать с ИИ» с Home-инсайтов). ChatView
   *  потребляет его один раз (заносит в поле ввода + фокус) и сбрасывает в ''. */
  draft: string;
  /** Переключает режим (нельзя во время стрима). */
  setMode: (mode: ChatMode) => void;
  /** Тоггл web-флага (нельзя во время стрима). Режим не трогает. */
  toggleWeb: () => void;
  /** AIP-3: задать предзаполнение композера (или '' для сброса после потребления). */
  setDraft: (text: string) => void;
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
  /** MEM-3 (AC-MEM-6): авто-ПРЕДЛОЖЕНИЕ факта после обмена (при `aiAgentMemory`) — кандидат от «быстрой»
   *  модели, ожидающий подтверждения чипом «Запомнить? ✓/✗». `null` — нет предложения. `messageId`
   *  привязывает чип к конкретному ответу (не показываем под устаревшим). */
  pendingFact: { messageId: string; text: string } | null;
  /** MEM-3: подтвердить предложенный факт → пишем в память агента (`source='auto'`), чип убираем.
   *  Промис реджектится при сбое записи (компонент показывает toast). */
  confirmFact: () => Promise<void>;
  /** MEM-3: отклонить предложенный факт — просто убрать чип, в БД НИЧЕГО не пишем (D1). */
  dismissFact: () => void;
  /** MEM-5: результат сразу-сохранения (явная команда «запомни …» / кнопка «В память»). ChatView
   *  показывает тост по `status` и сбрасывает. `saved` несёт `id` для «Отменить» (удаляем ТОЛЬКО реально
   *  созданный факт — `duplicate` уже был, без отмены; `error`/`nothing` — честные тосты). */
  savedFact: SaveResult | null;
  /** MEM-5: идёт извлечение факта по ЯВНОЙ команде (инлайн-индикатор «Сохраняю в память…»). */
  explicitSaving: boolean;
  /** MEM-5: id сообщения, для которого жмут «В память» (спиннер на кнопке); null — никакой. */
  capturingId: string | null;
  /** MEM-5: ChatView показал тост по `savedFact` → сбросить (одноразово). */
  acknowledgeSavedFact: () => void;
  /** MEM-5: «Отменить» только что сохранённый факт — удалить из памяти по id. */
  undoSavedFact: (id: number) => Promise<void>;
  /** MEM-5: кнопка «В память» под ответом — извлечь и СОХРАНИТЬ факт из обмена этого сообщения.
   *  Возвращает true, если что-то сохранено (для тоста «нечего сохранять» при false). */
  captureFromMessage: (messageId: string) => Promise<boolean>;
  /** Отправляет вопрос; `center` — путь открытого файла (граф-ранг в retrieval, только в vault-режиме). */
  send: (question: string, center?: string) => void;
  /** P6-RGN: перегенерировать последний ответ ИИ — тот же вопрос → свежий ответ. Старая пара
   *  убирается из ленты И из истории сессии (не двоим). No-op во время стрима / без обмена. */
  regenerate: (center?: string) => void;
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
  // Промис последнего персиста обмена (P6-RGN): regenerate ждёт его, чтобы sessionId уже был известен
  // (для ПЕРВОГО обмена он присваивается асинхронно — иначе чистка БД пропустится и вопрос задвоится).
  let lastSave: Promise<unknown> = Promise.resolve();

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
    lastSave = tauriApi.chat.sessions
      .logExchange(get().sessionId, ask.content, reply.content, sourcesJson)
      .then((sid) => set({ sessionId: sid }))
      .catch(() => {});
    void lastSave;
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

  // MEM-3 (AC-MEM-6): после завершённого обмена при `aiAgentMemory`=on просим «быструю» модель
  // предложить ≤1 факт-кандидат → чип подтверждения. Ничего НЕ пишем (D1). Best-effort: ошибка/нет
  // модели → молча без чипа. Гард на устаревание: показываем, только если `replyId` всё ещё последнее
  // сообщение и не стартовал новый стрим (иначе кандидат относится к прошлому обмену).
  const proposeFact = (replyId: string, userText: string, assistantText: string) => {
    if (!usePrefsStore.getState().aiAgentMemory) return;
    if (!userText.trim() || !assistantText.trim()) return;
    void tauriApi.memory
      .propose(userText, assistantText)
      .then((fact) => {
        const text = (fact ?? '').trim();
        if (!text) return;
        if (get().streaming) return; // уже идёт новый обмен
        if (get().messages.at(-1)?.id !== replyId) return; // ответ устарел
        set({ pendingFact: { messageId: replyId, text } });
      })
      .catch(() => {});
  };

  // MEM-5: ЯВНОЕ сохранение факта (команда «запомни …» или кнопка «В память»). Явная команда =
  // согласие (решение владельца) → пишем сразу, `source='explicit'`, без чипа-подтверждения. Различаем
  // исходы (adversarial-ревью): saved (создан, есть undo) / duplicate (уже был, БЕЗ undo — иначе стёрли
  // бы существующий факт) / error (запись упала — честный тост) / nothing (извлекать нечего).
  const runExplicitSave = async (
    userText: string,
    assistantText: string,
  ): Promise<SaveResult> => {
    let fact = (await tauriApi.memory.propose(userText, assistantText).catch(() => null))?.trim();
    if (!fact) fact = stripSaveCommand(userText); // фолбэк: срезать команду
    if (!fact) return { status: 'nothing' };
    try {
      const res = await tauriApi.memory.add(fact, 'explicit'); // {id, inserted} | null
      if (!res) return { status: 'nothing' }; // пустой текст (не должно при непустом fact)
      void useMemoryStore.getState().load(); // обновить панель «Память ИИ», если открыта
      return res.inserted
        ? { status: 'saved', id: res.id, text: fact }
        : { status: 'duplicate', text: fact };
    } catch {
      return { status: 'error' }; // реальный сбой записи — НЕ выдаём за «уже в памяти»
    }
  };

  return {
    messages: [],
    streaming: false,
    mode: 'vault',
    web: false,
    draft: '',
    pinned: [],
    sessionId: null,
    pendingFact: null,
    savedFact: null,
    explicitSaving: false,
    capturingId: null,

    confirmFact() {
      const pf = get().pendingFact;
      if (!pf) return Promise.resolve();
      set({ pendingFact: null });
      // Подтверждённое авто (D1): пишем с source='auto'. Промис наверх — компонент решает про toast.
      return tauriApi.memory.add(pf.text, 'auto').then(() => {});
    },
    dismissFact() {
      // Отказ (D1): ничего не пишем, просто снимаем чип.
      if (get().pendingFact) set({ pendingFact: null });
    },
    acknowledgeSavedFact() {
      if (get().savedFact) set({ savedFact: null });
    },
    undoSavedFact(id) {
      // «Отменить» сразу-сохранённый факт (явная команда/кнопка) — убрать из памяти.
      return tauriApi.memory
        .delete(id)
        .then(() => {
          void useMemoryStore.getState().load();
        })
        .catch(() => {});
    },
    async captureFromMessage(messageId) {
      if (get().streaming || get().capturingId) return false; // не во время стрима / двойного клика
      const msgs = get().messages;
      const idx = msgs.findIndex((m) => m.id === messageId);
      if (idx < 0 || msgs[idx].role !== 'assistant') return false;
      // Пара = ассистент-ответ + предшествующая реплика пользователя.
      const user = msgs.slice(0, idx).reverse().find((m) => m.role === 'user');
      const assistantText = msgs[idx].content;
      if (!user) return false;
      set({ capturingId: messageId });
      try {
        const res = await runExplicitSave(user.content, assistantText);
        // epoch-гард: за время await ленту могли очистить/сменить сессию — не вешаем тост на чужой экран.
        if (get().messages.some((m) => m.id === messageId)) set({ savedFact: res });
        return res.status === 'saved';
      } finally {
        set({ capturingId: null });
      }
    },

    setMode(mode) {
      if (get().streaming) return; // не переключаем режим на лету
      set({ mode });
    },
    setDraft(text) {
      set({ draft: text });
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
      // MEM-5: явная команда «запомни …» → сразу сохраним факт по завершении обмена (инлайн-индикатор).
      const explicit = isExplicitSave(q);

      const userMsg: ChatMessage = { id: nextId(), role: 'user', content: q };
      const replyId = nextId();
      const reply: ChatMessage = { id: replyId, role: 'assistant', content: '', streaming: true };
      pending = '';
      cancelFlush();
      // Новый обмен — снимаем прежнее авто-предложение факта (MEM-3), чтобы чип не висел над старым.
      set((s) => ({
        messages: [...s.messages, userMsg, reply],
        streaming: true,
        pendingFact: null,
        explicitSaving: explicit,
      }));

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
        // Epoch-гард (audit B12): принимаем события, только пока ИМЕННО этот ответ ещё стримится.
        // После stop()/нового send() сообщение replyId уже не streaming (или вытеснено) → поздние
        // токены старого стрима игнорируем, иначе они дописались бы в финализированный/чужой ответ.
        const cur = get().messages.find((m) => m.id === replyId);
        if (!cur || !cur.streaming) return;
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
            const answer = get().messages.find((m) => m.id === replyId)?.content ?? '';
            if (explicit) {
              // MEM-5: ЯВНАЯ команда = согласие → сохраняем сразу (source='explicit'), без чипа.
              void runExplicitSave(q, answer).then((res) => {
                // epoch-гард: за время propose+add юзер мог clear()/новый send()/сменить сессию —
                // не вешаем тост старого обмена на новую/очищенную ленту (как proposeFact).
                if (get().streaming || !get().messages.some((m) => m.id === replyId)) {
                  set({ explicitSaving: false });
                  return;
                }
                set({ savedFact: res, explicitSaving: false });
              });
            } else {
              // MEM-3 (AC-MEM-6): авто-предложение факта из обмена (если память агента включена) → чип.
              proposeFact(replyId, q, answer);
            }
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
            set({ streaming: false, explicitSaving: false }); // MEM-5: индикатор не висит при ошибке
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
        // MEM (AC-MEM-5): память агента — явные факты (пины + top-k). ВЫКЛ по умолчанию (D5).
        agentMemory: usePrefsStore.getState().aiAgentMemory,
        sessionId: get().sessionId,
        // P6-PIN: гарантированный контекст закреплённых заметок (полное содержимое).
        pinned: pinned.length ? pinned : undefined,
      });
    },

    regenerate(center) {
      if (get().streaming) return;
      const assistant = get().messages.at(-1);
      const user = get().messages.at(-2);
      if (
        !assistant ||
        !user ||
        assistant.role !== 'assistant' ||
        user.role !== 'user' ||
        assistant.streaming
      )
        return;
      const question = user.content;
      // Асинхронно: дождаться персиста прошлого обмена (sessionId присваивается в save() асинхронно —
      // для ПЕРВОГО обмена быстрый клик застал бы sessionId=null → чистка БД пропустилась бы и вопрос
      // задвоился). Ошибочный ответ НЕ персистится — его не ждём и не чистим (иначе снесли бы прошлый
      // хороший обмен). Затем подчищаем прошлую пару из истории и переспрашиваем тот же вопрос.
      void (async () => {
        if (!assistant.error) await lastSave;
        // За время await лента могла измениться (новый вопрос / стрим) — перепроверяем те же объекты.
        const m = get().messages;
        if (get().streaming || m.at(-1) !== assistant || m.at(-2) !== user) return;
        const sid = assistant.error ? null : get().sessionId;
        if (sid != null) {
          void tauriApi.chat.sessions
            .deleteLastExchange(sid)
            .catch(() => logUi('chat:regen-del-fail', `sid=${sid}`)); // фейл → деградация к append
        }
        set({ messages: m.slice(0, -2) });
        get().send(question, center); // режим/web/пины — текущие (как у обычного send)
      })();
    },

    stop() {
      cancelFn?.();
      cancelFn = null;
      cancelFlush();
      const tail = pending;
      pending = '';
      set((s) => ({
        streaming: false,
        explicitSaving: false, // MEM-5: прерванный обмен — индикатор сохранения не висит
        messages: s.messages.map((m) =>
          m.streaming ? { ...m, content: m.content + tail, streaming: false } : m,
        ),
      }));
      save();
    },

    clear() {
      disclosureOpen.clear();
      if (get().streaming) return;
      // MEM-5: сброс транзиентного состояния захвата (индикатор/чип не висят на пустой ленте).
      set({ messages: [], pendingFact: null, savedFact: null, explicitSaving: false, capturingId: null });
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
      set({
        messages: [],
        sessionId: null,
        pinned: [],
        pendingFact: null,
        savedFact: null,
        explicitSaving: false,
        capturingId: null,
      });
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
        // Перепроверка после await (audit B12): за время загрузки истории мог стартовать send()
        // (streaming=true). Без гарда set({messages: restored}) ниже затёр бы активный чат старой
        // историей — гонка «загрузка сессии vs новый вопрос».
        if (get().streaming) return;
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
        set({
          messages: restored,
          sessionId: id,
          pendingFact: null,
          savedFact: null,
          explicitSaving: false,
          capturingId: null,
        });
      } catch {
        /* сессия недоступна — лента не трогается */
      }
    },

    newSession() {
      if (get().streaming) return;
      logUi('chat:new-session');
      disclosureOpen.clear();
      set({
        messages: [],
        sessionId: null,
        pendingFact: null,
        savedFact: null,
        explicitSaving: false,
        capturingId: null,
      });
    },
  };
});
