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
 *
 * ⚠️ СЕМАНТИКА ОЧЕРЕДИ (ревью NB-1, CRITICAL-1/2): `newsfeed` — recurring-kind. После КАЖДОГО
 * завершения (и Ok, и fail) воркер тут же перепланирует следующий прогон (`reschedule_if_absent`,
 * scheduler.rs) → в steady state в очереди ВСЕГДА лежит pending «на завтра». Поэтому:
 *  - «джоба kind=newsfeed есть в activeJobs» ≠ «прогон идёт» — фильтруем ready-семантикой
 *    (`selectCurrentRun`: `running` ИЛИ `pending` с наступившим `run_at`), как Rust `has_ready_job`;
 *  - `jobActive` (Rust `is_kind_busy`: pending с ЛЮБЫМ run_at) для новостей НЕПРИГОДЕН — он и был
 *    корнем вечного «Собираю…» в шапке (CRITICAL-2): `load()` теперь считает `refreshing` тем же
 *    клиентским фильтром по `activeJobs`.
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
  /** ТЕКУЩИЙ прогон в очереди/выполняется («Собираю…» на кнопке) — ready-семантика, НЕ is_kind_busy. */
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
  /** NB-1: этап прогона из события `news:progress` (подписка живёт на уровне стора, MAJOR-1). */
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
/** Абсолютный потолок наблюдения (бэкенд-вотчдог тика оборвёт сам тик) → `stalled`. */
const RUNNING_CAP_MS = 20 * 60_000;
/** Прогресс не двигается дольше этого при живой джобе → кандидат в «похоже, зависло». */
const STUCK_MS = 120_000;
/** Гистерезис stuck (MINOR-1 ревью): подряд stuck-тиков до показа баннера — каденс этапов
 *  121–125с не мерцает предупреждением. */
const STUCK_TICKS = 2;
/** Зазор «pending вот-вот стартует» при выборе текущего прогона (только вперёд, в БУДУЩЕЕ):
 *  джоба, чей run_at наступает в пределах ближайшего опроса, уже считается текущей. */
const CURRENT_RUN_SLACK_MS = POLL_MS;

// ── Чистая склейка со снапшотом очереди (тестируется на реалистичных снапшотах, ревью NB-1) ────

/** Мини-форма активной джобы (структурное подмножество `ActiveJob`) — вход чистых функций. */
export interface QueueJob {
  id: number;
  kind: string;
  state: 'running' | 'pending';
  /** Unix-СЕКУНДЫ (как в `ActiveJob`). */
  runAt: number;
}

/** Мини-форма dead-джобы (структурное подмножество `DeadJob`). */
export interface QueueDead {
  id: number;
  kind: string;
  lastError: string | null;
  /** Unix-СЕКУНДЫ (как в `DeadJob`). */
  updatedAt: number;
}

/**
 * NB-1 (CRITICAL-1/2): выбирает из очереди джобу ТЕКУЩЕГО прогона новостей, отфильтровывая
 * «завтрашнюю» recurring-pending (у неё run_at в будущем). Семантика зеркалит Rust `has_ready_job`:
 * `running` ИЛИ `pending` с наступившим (± ближайший опрос) `run_at`. Если прогон уже отслеживается
 * (`trackedId`), держимся ЕГО id — так ретрай-бэкофф (pending с run_at в будущем) не теряется.
 */
export function selectCurrentRun(
  active: QueueJob[],
  trackedId: number | null,
  now: number,
): QueueJob | undefined {
  const news = active.filter((j) => j.kind === 'newsfeed');
  if (trackedId !== null) return news.find((j) => j.id === trackedId);
  return news.find((j) => j.state === 'running' || j.runAt * 1000 <= now + CURRENT_RUN_SLACK_MS);
}

/**
 * NB-1 (MAJOR-2): атрибуция смерти НАШЕМУ прогону. По id, если прогон отслеживался; иначе — только
 * dead-записи, умершие ПОСЛЕ старта наблюдения (`updatedAt >= startedAt`, БЕЗ обратного зазора:
 * прежний 30с-зазор ловил старую смерть как «нашу» после успешного ретрая).
 */
export function attributeDeath(
  dead: QueueDead[],
  trackedId: number | null,
  startedAtMs: number,
): QueueDead | undefined {
  const news = dead.filter((d) => d.kind === 'newsfeed');
  if (trackedId !== null) return news.find((d) => d.id === trackedId);
  return news
    .filter((d) => d.updatedAt * 1000 >= startedAtMs)
    .sort((a, b) => b.updatedAt - a.updatedAt)[0];
}

/** Решение вотчдога по снимку очереди (чистая функция — вся логика ливнеса тестируема без таймеров). */
export type RunDecision =
  | { kind: 'progressing' }
  | { kind: 'stuck' }
  | { kind: 'stalled' }
  | { kind: 'done' }
  | { kind: 'died'; reason: string | null };

/**
 * NB-1: чистое решение по ВЫБРАННОЙ джобе текущего прогона. Разводит «долго/живо» и «встало/умерло»:
 *  - текущей джобы нет + есть атрибутированная dead → `died` (причина = её `last_error`);
 *  - текущей джобы нет, dead нет → `done` (refetch: успех/дедуп);
 *  - `pending` дольше `PENDING_STALL_MS` ИЛИ наблюдение дольше `RUNNING_CAP_MS` → `stalled` (жёстко);
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

/**
 * NB-1: полная склейка «снимок очереди → решение» = `selectCurrentRun` + `attributeDeath` +
 * `evaluateRun` (ровно то, что делает тик вотчдога). Тестируется на РЕАЛИСТИЧНЫХ снапшотах —
 * в т.ч. очереди с recurring-pending «на завтра», которую живой планировщик держит всегда.
 * Возвращает решение и обновлённый `trackedId` (id найденной текущей джобы фиксируется).
 */
export function evaluateQueue(o: {
  active: QueueJob[];
  /** Dead-джобы; пустой список = не запрашивали (текущая джоба есть) либо dead нет. */
  dead: QueueDead[];
  trackedId: number | null;
  now: number;
  startedAt: number;
  lastProgressAt: number | null;
}): { decision: RunDecision; trackedId: number | null } {
  const job = selectCurrentRun(o.active, o.trackedId, o.now);
  const deadJob = job ? undefined : attributeDeath(o.dead, o.trackedId, o.startedAt);
  return {
    decision: evaluateRun({
      job,
      deadJob,
      now: o.now,
      startedAt: o.startedAt,
      lastProgressAt: o.lastProgressAt,
    }),
    trackedId: job ? job.id : o.trackedId,
  };
}

// Epoch-счётчик загрузок (audit B13): быстрая смена темы/«непрочитанные» во время in-flight load
// могла применить устаревший ответ (темы A) уже после переключения на B → лента не совпадала с чипом.
let loadEpoch = 0;

// ── Ливнес-вотчдог (NB-1): один цикл на прогон, разделяемый refresh()/load()/onProgress. Модульные,
// чтобы пережить пересоздание объекта состояния и гарантировать единственность цикла. ─────────────
/** Идёт ли уже цикл опроса (гард против двойного запуска). */
let watchdogActive = false;
/** Момент старта текущего цикла опроса (база для `RUNNING_CAP`/`STUCK`, атрибуция dead-джоб). */
let watchdogStartedAt = 0;
/** id джобы текущего прогона (CRITICAL-1: следим за КОНКРЕТНОЙ джобой, не за kind'ом). */
let trackedJobId: number | null = null;
/** Подряд идущих stuck-тиков (гистерезис MINOR-1). */
let stuckStreak = 0;
/** Момент последнего `news:progress` (движется ли прогон); `null` — событий ещё не было. */
let lastProgressAt: number | null = null;
/** Последний ненулевой этап (для атрибуции смерти: «прервалось на этапе X»). */
let lastStageName: string | null = null;

/** MAJOR-1: подписка `news:progress` живёт на уровне СТОРА (не вью) — уход со вкладки во время
 *  прогона больше не слепит вотчдог (ложное stuck) и не теряет этап для атрибуции смерти.
 *  Одна подписка на процесс, без снятия (слушатель дешёвый, стор — синглтон). */
let progressSubscribed = false;
function ensureProgressSubscription() {
  if (progressSubscribed) return;
  progressSubscribed = true;
  void tauriApi.events.onNewsProgress((p) => useNewsStore.getState().onProgress(p));
}

export const useNewsStore = create<NewsState>((set, get) => {
  /** Запускает цикл опроса очереди (только под Tauri — вне его реального планировщика нет). */
  const startWatchdog = () => {
    if (watchdogActive || !isTauri()) return;
    watchdogActive = true;
    watchdogStartedAt = Date.now();
    // Новое окно наблюдения: прежние прогресс-метки/id не относятся к этому прогону.
    trackedJobId = null;
    stuckStreak = 0;
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
        // MINOR-3: IPC-сбои тоже ограничены общим потолком — не перепланируем тик бесконечно.
        if (Date.now() - watchdogStartedAt > RUNNING_CAP_MS) {
          watchdogActive = false;
          set({ refreshing: false, stage: null, stuck: false, error: 'stalled' });
          return;
        }
        setTimeout(() => void tick(), POLL_MS);
        return;
      }
      // Dead-джобы нужны только когда текущей джобы не стало (done vs died).
      let dead: QueueDead[] = [];
      if (!selectCurrentRun(active, trackedJobId, Date.now())) {
        try {
          dead = await tauriApi.scheduler.deadJobs();
        } catch {
          /* нет доступа к dead → трактуем как завершение (done) */
        }
      }
      const { decision, trackedId } = evaluateQueue({
        active,
        dead,
        trackedId: trackedJobId,
        now: Date.now(),
        startedAt: watchdogStartedAt,
        lastProgressAt,
      });
      trackedJobId = trackedId;
      switch (decision.kind) {
        case 'died':
          watchdogActive = false;
          // Прошлые данные целы (как W-2/errorSub) — поверх них честный баннер этапа+причины.
          set({
            refreshing: false,
            stage: null,
            stuck: false,
            died: { stage: lastStageName, reason: decision.reason },
          });
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
          // Гистерезис (MINOR-1): один stuck-тик на границе порога не мерцает баннером.
          stuckStreak += 1;
          if (stuckStreak >= STUCK_TICKS && !get().stuck) set({ stuck: true });
          setTimeout(() => void tick(), POLL_MS);
          return;
        case 'progressing':
          stuckStreak = 0;
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
      ensureProgressSubscription();
      const epoch = ++loadEpoch;
      try {
        const { topic, unreadOnly } = get();
        const [config, sources, page, activeList] = await Promise.all([
          tauriApi.news.getConfig(),
          tauriApi.news.sources(),
          tauriApi.news.page({ topic: topic ?? undefined, unreadOnly }),
          tauriApi.scheduler.activeJobs(),
        ]);
        // CRITICAL-2 (корень жалобы владельца): `jobActive` (Rust is_kind_busy) считает и «завтрашнюю»
        // recurring-pending → в steady state спиннер «Собираю…» горел ВЕЧНО. Ready-семантика
        // (selectCurrentRun) считает прогоном только running / pending с наступившим run_at.
        const stillRefreshing = selectCurrentRun(activeList, null, Date.now()) !== undefined;
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
          // MINOR-2: живой прогон обнаружен (напр. ретрай dead-джобы из модалки очереди) →
          // прежний баннер «прервалось» неактуален.
          ...(stillRefreshing ? { died: null } : {}),
        });
        // NB-1: прогон уже идёт (плановый суточный тик / ретрай из модалки / прогон, запущенный при
        // закрытой странице) — поднимаем ливнес-вотчдог, чтобы «зависло/умерло» отслеживалось всегда.
        if (stillRefreshing) startWatchdog();
      } catch (e) {
        if (epoch !== loadEpoch) return; // устаревшая загрузка не показывает свою ошибку
        set({ loading: false, refreshing: false, error: String(e) });
      }
    },

    refresh: async () => {
      if (get().refreshing) return;
      ensureProgressSubscription();
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
        // «встало/умерло» через чистую `evaluateQueue`. Текущей джобы нет → done|died; pending>минуты
        // или наблюдение>потолка → stalled; running без движения прогресса > STUCK → мягкое «зависло».
        startWatchdog();
      } catch (e) {
        set({ refreshing: false, error: String(e) });
      }
    },

    onProgress: (p) => {
      lastProgressAt = Date.now();
      stuckStreak = 0;
      const live = p.stage !== 'save';
      if (live) lastStageName = p.stage;
      // Живой этап пришёл → прогон точно идёт и двигается: снимаем «зависло»/смерть; если прогон
      // начался планово (не через refresh) — взводим refreshing и вотчдог, чтобы статус был живым.
      set({
        stage: live ? p : null,
        stuck: false,
        died: null,
        ...(live && !get().refreshing ? { refreshing: true } : {}),
      });
      if (live) startWatchdog(); // no-op, если цикл уже идёт / вне Tauri
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
