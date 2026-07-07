import { create } from 'zustand';
import {
  isTauri,
  tauriApi,
  type NewsConfig,
  type NewsItem,
  type NewsRun,
  type NewsSource,
} from '../lib/tauri-api';

/**
 * Состояние страницы «Новости» (NF-5, спека `docs/specs/news-feed.md`). Данные — целиком
 * бэкенд-команды NF-3 (страница/конфиг/реестр); фильтры тема/непрочитанные — серверные
 * (повторный `load`). Прогон асинхронный: `refresh()` ставит джобу, результат прилетает
 * по `jobs:changed` (refetch в NewsView). `config.enabled` = consent (AC-NF-7).
 *
 * NB-1 (замечание владельца 2026-07-08 «непонятен статус: долгая обработка или что-то сломалось»):
 * ЖИВОЙ статус прогона поверх существующей джоб-инфры БЕЗ новой шины. Три сигнала:
 *  - `stage` — текущий этап прогона (`news:progress`, эмитится пайплайном run.rs: sources → llm →
 *    digest → save); показывает «Опрашиваю источники 7/16» / «Анализирую записи 12/40».
 *  - `stuck` — МЯГКОЕ «похоже, зависло»: прогресс не двигается дольше `STUCK_MS` при живой джобе.
 *    Отличает «долго» от «встало» (главная жалоба). Не убивает джобу — только предупреждает.
 *  - `died` — ЧЕСТНАЯ ошибка ЭТАПА: джоба ушла в dead → показываем, на каком этапе (последний
 *    виденный `stage`) и почему (`last_error` мёртвой джобы). Не дублирует W-2-баннер llmDown
 *    (тот — Ok-прогон с записью run; здесь — именно упавшая джоба).
 */
interface NewsState {
  items: NewsItem[];
  topics: string[];
  run: NewsRun | null;
  config: NewsConfig | null;
  sources: NewsSource[];
  /** Активный чип темы (`null` — «Все»). */
  topic: string | null;
  unreadOnly: boolean;
  /** Первая загрузка страницы (до прихода данных). */
  loading: boolean;
  /** Джоба прогона в очереди/выполняется («Собираю…» на кнопке). */
  refreshing: boolean;
  /** NB-1: текущий этап живого прогона (`null` — прогон не идёт / завершился этапом `save`). */
  stage: RunStage | null;
  /** NB-1: прогресс не двигается дольше `STUCK_MS` при живой джобе — мягкое «похоже, зависло». */
  stuck: boolean;
  /** NB-1: джоба прогона умерла — на каком этапе (последний виденный) и почему (`last_error`). */
  died: RunDeath | null;
  error: string | null;
  /** Транзиентное уведомление («Создана заметка …»); самосбрасывается в NewsView. */
  notice: string | null;
  load: () => Promise<void>;
  /** Ручной прогон (AC-NF-6): дедуп на бэке; снимается по `jobs:changed`-refetch. */
  refresh: () => Promise<void>;
  /** NB-1: этап прогона из события `news:progress` (зовёт подписчик в NewsView). */
  onProgress: (p: RunStage) => void;
  markRead: (id: number, read: boolean) => Promise<void>;
  toNote: (id: number) => Promise<void>;
  /** Вкл/выкл ленты (consent): пишет news.json и мгновенно правит политику эгресса (NF-4). */
  setEnabled: (enabled: boolean) => Promise<void>;
  setTopic: (topic: string | null) => void;
  setUnreadOnly: (unreadOnly: boolean) => void;
  clearNotice: () => void;
}

/** Этап живого прогона (полезная нагрузка `news:progress`, зеркалит Rust `NewsProgress`). */
export interface RunStage {
  /** `sources` | `llm` | `digest` | `save` (последний очищает `stage`). */
  stage: string;
  done: number;
  total: number;
}

/** NB-1: атрибутированная этапу смерть прогона (для честного баннера вместо вечного «Собираю…»). */
export interface RunDeath {
  /** Этап, на котором джоба умерла (последний виденный `stage`); `null` — этап не наблюдался. */
  stage: string | null;
  /** `last_error` мёртвой джобы; `null` — причина не записана. */
  reason: string | null;
}

// ── Пороги вотчдога (NB-1). Консервативные: «долго» ≠ «зависло». ──────────────────────────────
/** Интервал опроса очереди планировщика. */
const POLL_MS = 5000;
/** `pending` дольше этого без запуска → планировщик мёртв (честная ошибка `stalled`, инцидент 06-12). */
const PENDING_STALL_MS = 60_000;
/** Абсолютный потолок `running` (бэкенд-вотчдог тика оборвёт сам тик) → `stalled`. */
const RUNNING_CAP_MS = 20 * 60_000;
/** Прогресс не двигается дольше этого при живой джобе → мягкое «похоже, зависло» (не ошибка). */
const STUCK_MS = 120_000;
/** Насколько «назад» от старта вотчдога dead-джоба считается смертью ИМЕННО этого прогона. */
const DEAD_SLACK_MS = 30_000;

/** Решение вотчдога по снимку очереди (чистая функция — вся логика ливнеса тестируема без таймеров). */
export type RunDecision =
  | { kind: 'progressing' }
  | { kind: 'stuck' }
  | { kind: 'stalled' }
  | { kind: 'done' }
  | { kind: 'died'; reason: string | null };

/**
 * NB-1: чистое решение по одному снимку очереди. Разводит «долго/живо» и «встало/умерло»:
 *  - нет активной джобы + есть свежая dead-джоба → `died` (причина = её `last_error`);
 *  - нет активной джобы, dead нет → `done` (refetch: успех/дедуп);
 *  - `pending` дольше `PENDING_STALL_MS` ИЛИ `running` дольше `RUNNING_CAP_MS` → `stalled` (жёстко);
 *  - `running` без движения прогресса дольше `STUCK_MS` → `stuck` (мягко, опрос продолжается);
 *  - иначе → `progressing`.
 * `lastProgressAt` = момент последнего `news:progress`; `null` → отсчёт от старта вотчдога (джоба
 * бежит, но ни одного этапа ещё не прислала — тоже кандидат в «зависло» по тому же порогу).
 */
export function evaluateRun(o: {
  job: { state: 'running' | 'pending'; runAt: number } | undefined;
  deadJob: { lastError: string | null } | undefined;
  now: number;
  startedAt: number;
  lastProgressAt: number | null;
}): RunDecision {
  const { job, deadJob, now, startedAt, lastProgressAt } = o;
  if (!job) {
    if (deadJob) return { kind: 'died', reason: deadJob.lastError };
    return { kind: 'done' };
  }
  if (job.state === 'pending' && now - job.runAt * 1000 > PENDING_STALL_MS) return { kind: 'stalled' };
  if (now - startedAt > RUNNING_CAP_MS) return { kind: 'stalled' };
  if (job.state === 'running' && now - (lastProgressAt ?? startedAt) > STUCK_MS) {
    return { kind: 'stuck' };
  }
  return { kind: 'progressing' };
}

// Epoch-счётчик загрузок (audit B13): быстрая смена темы/«непрочитанные» во время in-flight load
// могла применить устаревший ответ (темы A) уже после переключения на B → лента не совпадала с чипом.
let loadEpoch = 0;

// ── Ливнес-вотчдог (NB-1): один цикл на прогон, разделяемый refresh()/load(). Модульные, чтобы
// пережить пересоздание объекта состояния и гарантировать единственность цикла. ──────────────────
/** Идёт ли уже цикл опроса (гард против двойного запуска из refresh()+load()). */
let watchdogActive = false;
/** Момент старта текущего цикла опроса (база для `RUNNING_CAP`/`STUCK`, атрибуция dead-джоб). */
let watchdogStartedAt = 0;
/** Момент последнего `news:progress` (движется ли прогон); `null` — событий ещё не было. */
let lastProgressAt: number | null = null;
/** Последний ненулевой этап (для атрибуции смерти: «прервалось на этапе X»). */
let lastStageName: string | null = null;

export const useNewsStore = create<NewsState>((set, get) => {
  /** Запускает цикл опроса очереди (только под Tauri — вне его реального планировщика нет). */
  const startWatchdog = () => {
    if (watchdogActive || !isTauri()) return;
    watchdogActive = true;
    watchdogStartedAt = Date.now();
    // Новое окно наблюдения: прежние прогресс-метки не относятся к этому прогону.
    lastProgressAt = null;
    lastStageName = null;

    const tick = async () => {
      if (!get().refreshing) {
        watchdogActive = false;
        return;
      }
      let active;
      try {
        active = await tauriApi.scheduler.activeJobs();
      } catch {
        setTimeout(() => void tick(), POLL_MS);
        return;
      }
      const job = active.find((j) => j.kind === 'newsfeed');
      let deadJob;
      if (!job) {
        let dead: Awaited<ReturnType<typeof tauriApi.scheduler.deadJobs>> = [];
        try {
          dead = await tauriApi.scheduler.deadJobs();
        } catch {
          /* нет доступа к dead → трактуем как завершение (done) */
        }
        deadJob = dead
          .filter(
            (d) => d.kind === 'newsfeed' && d.updatedAt * 1000 >= watchdogStartedAt - DEAD_SLACK_MS,
          )
          .sort((a, b) => b.updatedAt - a.updatedAt)[0];
      }
      const decision = evaluateRun({
        job,
        deadJob,
        now: Date.now(),
        startedAt: watchdogStartedAt,
        lastProgressAt,
      });
      switch (decision.kind) {
        case 'died':
          watchdogActive = false;
          // Прошлые данные целы (как W-2/errorSub) — поверх них честный баннер этапа+причины.
          set({ refreshing: false, stage: null, stuck: false, died: { stage: lastStageName, reason: decision.reason } });
          return;
        case 'done':
          watchdogActive = false;
          // Прогон завершился успешно → снимаем сигналы прошлой неудачи (ретрай после смерти/зависания).
          set({ stuck: false, died: null });
          void get().load();
          return;
        case 'stalled':
          watchdogActive = false;
          set({ refreshing: false, stage: null, stuck: false, error: 'stalled' });
          return;
        case 'stuck':
          if (!get().stuck) set({ stuck: true });
          setTimeout(() => void tick(), POLL_MS);
          return;
        case 'progressing':
          if (get().stuck) set({ stuck: false });
          setTimeout(() => void tick(), POLL_MS);
          return;
      }
    };
    setTimeout(() => void tick(), POLL_MS);
  };

  return {
    items: [],
    topics: [],
    run: null,
    config: null,
    sources: [],
    topic: null,
    unreadOnly: false,
    loading: true,
    refreshing: false,
    stage: null,
    stuck: false,
    died: null,
    error: null,
    notice: null,

    load: async () => {
      const epoch = ++loadEpoch;
      try {
        const { topic, unreadOnly } = get();
        const [config, sources, page] = await Promise.all([
          tauriApi.news.getConfig(),
          tauriApi.news.sources(),
          tauriApi.news.page({ topic: topic ?? undefined, unreadOnly }),
        ]);
        const stillRefreshing = await tauriApi.scheduler.jobActive('newsfeed');
        if (epoch !== loadEpoch) return; // фильтр сменился во время загрузки → этот ответ устарел
        set({
          config,
          sources,
          items: page.items,
          topics: page.topics,
          run: page.run,
          loading: false,
          refreshing: stillRefreshing,
          error: null,
        });
        // NB-1: прогон уже идёт (плановый суточный тик или тот, что запустили при закрытой странице) —
        // поднимаем ливнес-вотчдог, чтобы «зависло/умерло» отслеживалось и вне ручного «Обновить».
        if (stillRefreshing) startWatchdog();
      } catch (e) {
        if (epoch !== loadEpoch) return; // устаревшая загрузка не показывает свою ошибку
        set({ loading: false, refreshing: false, error: String(e) });
      }
    },

    refresh: async () => {
      if (get().refreshing) return;
      // Новый прогон: гасим прежние сигналы прошлого прогона (этап/зависание/смерть/ошибку).
      set({ refreshing: true, error: null, stage: null, stuck: false, died: null });
      try {
        await tauriApi.news.refresh();
        // Вне Tauri событий `jobs:changed` нет — мок «завершает прогон» отложенным refetch'ом (сам
        // мок при этом эмитит `news:progress`, так что живой этап в браузер-превью виден).
        if (!isTauri()) {
          setTimeout(() => void get().load(), 1500);
          return;
        }
        // Ливнес-вотчдог (инцидент 2026-06-12 + NB-1): поллит очередь и разводит «долго/живо» vs
        // «встало/умерло» через чистую `evaluateRun`. Джобы нет → done|died; pending>минуты или
        // running>потолка → stalled; running без движения прогресса > STUCK → мягкое «зависло».
        startWatchdog();
      } catch (e) {
        set({ refreshing: false, error: String(e) });
      }
    },

    onProgress: (p) => {
      lastProgressAt = Date.now();
      if (p.stage !== 'save') lastStageName = p.stage;
      // Живой этап пришёл → прогон точно двигается: снимаем «зависло» и любую атрибутированную смерть.
      set({ stage: p.stage === 'save' ? null : p, stuck: false, died: null });
    },

    markRead: async (id, read) => {
      // Оптимистично: карточка тускнеет сразу, бэкенд догоняет (ошибка — откат через load).
      set((s) => ({ items: s.items.map((it) => (it.id === id ? { ...it, read } : it)) }));
      try {
        await tauriApi.news.markRead(id, read);
      } catch (e) {
        set({ error: String(e) });
        void get().load();
      }
    },

    toNote: async (id) => {
      try {
        const path = await tauriApi.news.toNote(id);
        set({ notice: path });
      } catch (e) {
        set({ error: String(e) });
      }
    },

    setEnabled: async (enabled) => {
      const current = get().config ?? {
        enabled: false,
        sources: {},
        keywords: null,
        extraHosts: [],
        modelPref: null,
      };
      try {
        const config = await tauriApi.news.setConfig({ ...current, enabled });
        set({ config });
        if (enabled) {
          // Первый прогон сразу после consent — лента наполняется без ожидания суточного тика.
          await get().refresh();
          await get().load();
        }
      } catch (e) {
        set({ error: String(e) });
      }
    },

    setTopic: (topic) => {
      set({ topic });
      void get().load();
    },

    setUnreadOnly: (unreadOnly) => {
      set({ unreadOnly });
      void get().load();
    },

    clearNotice: () => set({ notice: null }),
  };
});
