import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../lib/tauri-api';
import {
  __setWatchdogStateForTest,
  evaluateQueue,
  evaluateRun,
  selectCurrentRun,
  useNewsStore,
} from './news';

afterEach(() => {
  vi.restoreAllMocks();
  __setWatchdogStateForTest({ active: false, trackedId: null });
  useNewsStore.setState({
    items: [],
    topic: null,
    unreadOnly: false,
    loading: true,
    refreshing: false,
    error: null,
    stage: null,
    stuck: false,
    died: null,
  });
});

describe('news store — epoch-гард load (audit B13)', () => {
  it('устаревшая загрузка не затирает свежую', async () => {
    let call = 0;
    // Каждый вызов page() отдаёт свой набор; обе load() стартуют синхронно (++loadEpoch), поэтому
    // первая (epoch 1) после await заведомо устарела относительно второй (epoch 2) и должна быть отброшена.
    vi.spyOn(tauriApi.news, 'page').mockImplementation(
      () => Promise.resolve({ items: [{ id: ++call }], topics: [], run: null }) as never,
    );
    vi.spyOn(tauriApi.news, 'getConfig').mockResolvedValue({} as never);
    vi.spyOn(tauriApi.news, 'sources').mockResolvedValue([] as never);
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([]);

    const p1 = useNewsStore.getState().load(); // epoch 1 (тема A)
    const p2 = useNewsStore.getState().load(); // epoch 2 (тема B — вытесняет)
    await Promise.all([p1, p2]);

    // Применился только результат свежей загрузки (второй вызов page).
    expect(useNewsStore.getState().items.map((i) => i.id)).toEqual([2]);
  });

  it('одиночная загрузка применяется нормально (гард не мешает)', async () => {
    vi.spyOn(tauriApi.news, 'page').mockResolvedValue({
      items: [{ id: 7 }],
      topics: [],
      run: null,
    } as never);
    vi.spyOn(tauriApi.news, 'getConfig').mockResolvedValue({} as never);
    vi.spyOn(tauriApi.news, 'sources').mockResolvedValue([] as never);
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([]);

    await useNewsStore.getState().load();
    expect(useNewsStore.getState().items.map((i) => i.id)).toEqual([7]);
    expect(useNewsStore.getState().loading).toBe(false);
  });
});

// ── NB-1 (ревью, CRITICAL-1/2): склейка со СНАПШОТОМ очереди. Реалистичные снапшоты: newsfeed —
// recurring, после каждого завершения в очереди ВСЕГДА лежит pending «на завтра» (reschedule_if_absent
// в том же проходе воркера) — наивный поиск по kind её и находил, убивая ветки done/died. ────────────
describe('news store — selectCurrentRun/evaluateQueue (склейка со снапшотом очереди)', () => {
  const NOW = 1_800_000_000_000; // «сейчас» (мс)
  const sec = (ms: number) => Math.floor(ms / 1000);
  /** Recurring-джоба следующего суточного прогона — фон КАЖДОГО steady-state снапшота. */
  const tomorrow = { id: 7, kind: 'newsfeed', state: 'pending' as const, runAt: sec(NOW) + 86_400 };

  it('CRITICAL-2 (корень вечного «Собираю…»): steady state — только «завтрашняя» pending → прогона НЕТ', () => {
    expect(selectCurrentRun([tomorrow], null, NOW)).toBeUndefined();
  });

  it('running-джоба на фоне «завтрашней» → текущий прогон, id фиксируется', () => {
    const running = { id: 42, kind: 'newsfeed', state: 'running' as const, runAt: sec(NOW) - 10 };
    const r = evaluateQueue({
      active: [tomorrow, running],
      dead: [],
      trackedId: null,
      now: NOW,
      startedAt: NOW - 10_000,
      lastProgressAt: NOW - 5_000,
    });
    expect(r.decision).toEqual({ kind: 'progressing' });
    expect(r.trackedId).toBe(42);
  });

  it('pending с наступившим run_at (ручной прогон ещё не заклеймлен) → текущий прогон', () => {
    const manual = { id: 43, kind: 'newsfeed', state: 'pending' as const, runAt: sec(NOW) - 1 };
    expect(selectCurrentRun([tomorrow, manual], null, NOW)?.id).toBe(43);
  });

  it('чужой kind (digest и т.п.) не участвует в выборе', () => {
    const digest = { id: 9, kind: 'digest', state: 'running' as const, runAt: sec(NOW) };
    expect(selectCurrentRun([digest, tomorrow], null, NOW)).toBeUndefined();
  });

  it('CRITICAL-1: done ДОСТИЖИМ — прогон завершился, в очереди осталась лишь «завтрашняя»', () => {
    const { decision } = evaluateQueue({
      active: [tomorrow], // прежний фильтр по kind нашёл бы её и никогда не дал done
      dead: [],
      trackedId: 42,
      now: NOW,
      startedAt: NOW - 60_000,
      lastProgressAt: NOW - 30_000,
    });
    expect(decision).toEqual({ kind: 'done' });
  });

  it('CRITICAL-1: died ДОСТИЖИМ — наша джоба в dead при «завтрашней» pending в очереди', () => {
    const dead = [{ id: 42, kind: 'newsfeed', lastError: 'llm: connection refused', updatedAt: sec(NOW) }];
    const { decision } = evaluateQueue({
      active: [tomorrow],
      dead,
      trackedId: 42, // id зафиксирован, пока джоба была видна running
      now: NOW,
      startedAt: NOW - 60_000,
      lastProgressAt: NOW - 30_000,
    });
    expect(decision).toEqual({ kind: 'died', reason: 'llm: connection refused' });
  });

  it('MAJOR-2: чужая dead (умерла ДО старта наблюдения) НЕ атрибутируется → done, не ложный died', () => {
    // Сценарий ревью: прогон умер → «Обновить» → успех; в dead осталась СТАРАЯ запись.
    const dead = [{ id: 41, kind: 'newsfeed', lastError: 'старая смерть', updatedAt: sec(NOW) - 20 }];
    const { decision } = evaluateQueue({
      active: [tomorrow],
      dead,
      trackedId: null, // новый прогон завершился до первого тика — id не успел зафиксироваться
      now: NOW,
      startedAt: NOW, // наблюдение началось ПОСЛЕ старой смерти
      lastProgressAt: null,
    });
    expect(decision).toEqual({ kind: 'done' });
  });

  it('без tracked id свежая (после старта наблюдения) dead-джоба атрибутируется', () => {
    const dead = [{ id: 44, kind: 'newsfeed', lastError: 'паника хендлера', updatedAt: sec(NOW) }];
    const { decision } = evaluateQueue({
      active: [tomorrow],
      dead,
      trackedId: null,
      now: NOW,
      startedAt: NOW - 30_000,
      lastProgressAt: null,
    });
    expect(decision).toEqual({ kind: 'died', reason: 'паника хендлера' });
  });

  it('tracked id держит прогон и на ретрай-бэкоффе (pending с run_at в будущем)', () => {
    // fail() перекладывает джобу в pending с отложенным run_at — это ЕЩЁ наш прогон, не «завтрашняя».
    const retrying = { id: 42, kind: 'newsfeed', state: 'pending' as const, runAt: sec(NOW) + 60 };
    const r = evaluateQueue({
      active: [tomorrow, retrying],
      dead: [],
      trackedId: 42,
      now: NOW,
      startedAt: NOW - 30_000,
      lastProgressAt: NOW - 10_000,
    });
    expect(r.decision).toEqual({ kind: 'progressing' });
    expect(r.trackedId).toBe(42);
  });
});

// NB-1: ливнес-решение вотчдога — чистая функция, разводит «долго/живо» и «встало/умерло».
// (Юниты внутреннего решения; реалистичная склейка со снапшотом очереди — describe выше.)
describe('news store — evaluateRun (NB-1 ливнес прогона)', () => {
  const T0 = 1_000_000_000_000; // база «сейчас» (мс)

  it('running с недавним прогрессом → progressing', () => {
    const d = evaluateRun({
      job: { state: 'running', runAt: T0 / 1000 },
      deadJob: undefined,
      now: T0 + 30_000,
      startedAt: T0,
      lastProgressAt: T0 + 20_000, // 10с назад — движется
    });
    expect(d).toEqual({ kind: 'progressing' });
  });

  it('running без движения прогресса дольше 120с → stuck (мягко)', () => {
    const d = evaluateRun({
      job: { state: 'running', runAt: T0 / 1000 },
      deadJob: undefined,
      now: T0 + 130_000,
      startedAt: T0,
      lastProgressAt: T0, // 130с без нового этапа
    });
    expect(d).toEqual({ kind: 'stuck' });
  });

  it('running и НИ ОДНОГО прогресс-события дольше 120с → stuck (отсчёт от старта)', () => {
    const d = evaluateRun({
      job: { state: 'running', runAt: T0 / 1000 },
      deadJob: undefined,
      now: T0 + 130_000,
      startedAt: T0,
      lastProgressAt: null,
    });
    expect(d).toEqual({ kind: 'stuck' });
  });

  it('pending дольше 60с → stalled (планировщик не запускает)', () => {
    const d = evaluateRun({
      job: { state: 'pending', runAt: T0 / 1000 - 70 }, // готова 70с назад, не бежит
      deadJob: undefined,
      now: T0,
      startedAt: T0,
      lastProgressAt: null,
    });
    expect(d).toEqual({ kind: 'stalled' });
  });

  it('running дольше абсолютного потолка (20 мин) → stalled', () => {
    const d = evaluateRun({
      job: { state: 'running', runAt: T0 / 1000 },
      deadJob: undefined,
      now: T0 + 21 * 60_000,
      startedAt: T0,
      lastProgressAt: T0 + 21 * 60_000, // прогресс «свежий», но потолок превышен
    });
    expect(d).toEqual({ kind: 'stalled' });
  });

  it('джобы нет + свежая dead → died с её причиной', () => {
    const d = evaluateRun({
      job: undefined,
      deadJob: { lastError: 'insert_items: disk I/O error' },
      now: T0,
      startedAt: T0,
      lastProgressAt: T0,
    });
    expect(d).toEqual({ kind: 'died', reason: 'insert_items: disk I/O error' });
  });

  it('джобы нет, dead нет → done (успех/дедуп, refetch)', () => {
    const d = evaluateRun({
      job: undefined,
      deadJob: undefined,
      now: T0,
      startedAt: T0,
      lastProgressAt: T0,
    });
    expect(d).toEqual({ kind: 'done' });
  });
});

// NB-1: onProgress — этап живого прогона в стор; снимает «зависло/умерло» (прогон точно двигается).
describe('news store — onProgress (NB-1 живой этап)', () => {
  it('этап sources/llm → stage выставлен, stuck/died сняты, refreshing взведён (плановый прогон)', () => {
    useNewsStore.setState({ stuck: true, died: { stage: 'llm', reason: 'x' }, refreshing: false });
    useNewsStore.getState().onProgress({ stage: 'llm', done: 12, total: 40 });
    const s = useNewsStore.getState();
    expect(s.stage).toEqual({ stage: 'llm', done: 12, total: 40 });
    expect(s.stuck).toBe(false);
    expect(s.died).toBeNull();
    // Плановый (не через «Обновить») прогон тоже показывает живой статус в шапке.
    expect(s.refreshing).toBe(true);
  });

  it('этап save → stage очищается (прогон завершил этапы), refreshing НЕ взводится', () => {
    useNewsStore.getState().onProgress({ stage: 'save', done: 1, total: 1 });
    expect(useNewsStore.getState().stage).toBeNull();
    expect(useNewsStore.getState().refreshing).toBe(false);
  });
});

// NB-1 (MINOR-2 ревью): ретрай dead-джобы из модалки очереди → jobs:changed → load(): живой прогон
// обнаружен → баннер «прервалось» снимается (иначе «прервалось» и «Собираю…» висели бы вместе).
describe('news store — load() при живом прогоне (NB-1)', () => {
  it('живой прогон в очереди → refreshing=true и died снят', async () => {
    vi.spyOn(tauriApi.news, 'page').mockResolvedValue({ items: [], topics: [], run: null } as never);
    vi.spyOn(tauriApi.news, 'getConfig').mockResolvedValue({} as never);
    vi.spyOn(tauriApi.news, 'sources').mockResolvedValue([] as never);
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      { id: 5, kind: 'newsfeed', state: 'running', runAt: Math.floor(Date.now() / 1000), attempts: 1 },
    ]);
    useNewsStore.setState({ died: { stage: 'llm', reason: 'умерла в прошлый раз' } });

    await useNewsStore.getState().load();
    expect(useNewsStore.getState().refreshing).toBe(true);
    expect(useNewsStore.getState().died).toBeNull();
  });

  it('steady state («завтрашняя» pending) → refreshing=false, died прошлого прогона сохранён', async () => {
    vi.spyOn(tauriApi.news, 'page').mockResolvedValue({ items: [], topics: [], run: null } as never);
    vi.spyOn(tauriApi.news, 'getConfig').mockResolvedValue({} as never);
    vi.spyOn(tauriApi.news, 'sources').mockResolvedValue([] as never);
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      {
        id: 7,
        kind: 'newsfeed',
        state: 'pending',
        runAt: Math.floor(Date.now() / 1000) + 86_400,
        attempts: 0,
      },
    ]);
    useNewsStore.setState({ died: { stage: 'llm', reason: 'актуальная смерть' } });

    await useNewsStore.getState().load();
    // CRITICAL-2: «завтрашняя» recurring-pending НЕ зажигает вечный спиннер.
    expect(useNewsStore.getState().refreshing).toBe(false);
    // Смерть последнего прогона остаётся видимой (её снимает только новый живой прогон/refresh).
    expect(useNewsStore.getState().died).toEqual({ stage: 'llm', reason: 'актуальная смерть' });
  });

  // MINOR-A (ревью NB-1): вотчдог держит бэкофф-джобу по id (pending с run_at за 5с-зазором) →
  // load() от чужого jobs:changed (gc/digest) обязан видеть ЕЁ ЖЕ, а не гасить refreshing
  // (иначе цикл умирает и смерть attempt-2 до первого progress-события теряется молча).
  it('вотчдог держит бэкофф-джобу по id → load() не гасит refreshing', async () => {
    vi.spyOn(tauriApi.news, 'page').mockResolvedValue({ items: [], topics: [], run: null } as never);
    vi.spyOn(tauriApi.news, 'getConfig').mockResolvedValue({} as never);
    vi.spyOn(tauriApi.news, 'sources').mockResolvedValue([] as never);
    const nowSec = Math.floor(Date.now() / 1000);
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      // «Завтрашняя» recurring + НАША джоба на ретрай-бэкоффе (run_at +60с — вне ready-зазора).
      { id: 7, kind: 'newsfeed', state: 'pending', runAt: nowSec + 86_400, attempts: 0 },
      { id: 42, kind: 'newsfeed', state: 'pending', runAt: nowSec + 60, attempts: 1 },
    ]);
    __setWatchdogStateForTest({ active: true, trackedId: 42 });
    useNewsStore.setState({ refreshing: true });

    await useNewsStore.getState().load();
    // Ready-фильтр без id дал бы «прогона нет» — с tracked id наблюдение сохраняется.
    expect(useNewsStore.getState().refreshing).toBe(true);

    // Контраст: без активного вотчдога тот же снапшот честно гасит спиннер (бэкофф-джоба
    // анонимному load() неизвестна — подхватится вотчдогом/довершится jobs:changed'ом).
    __setWatchdogStateForTest({ active: false, trackedId: null });
    await useNewsStore.getState().load();
    expect(useNewsStore.getState().refreshing).toBe(false);
  });
});
