import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../lib/tauri-api';
import { evaluateRun, useNewsStore } from './news';

afterEach(() => {
  vi.restoreAllMocks();
  useNewsStore.setState({
    items: [],
    topic: null,
    unreadOnly: false,
    loading: true,
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
    vi.spyOn(tauriApi.scheduler, 'jobActive').mockResolvedValue(false as never);

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
    vi.spyOn(tauriApi.scheduler, 'jobActive').mockResolvedValue(false as never);

    await useNewsStore.getState().load();
    expect(useNewsStore.getState().items.map((i) => i.id)).toEqual([7]);
    expect(useNewsStore.getState().loading).toBe(false);
  });
});

// NB-1: ливнес-решение вотчдога — чистая функция, разводит «долго/живо» и «встало/умерло».
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
  it('этап sources/llm → stage выставлен, stuck/died сняты', () => {
    useNewsStore.setState({ stuck: true, died: { stage: 'llm', reason: 'x' } });
    useNewsStore.getState().onProgress({ stage: 'llm', done: 12, total: 40 });
    const s = useNewsStore.getState();
    expect(s.stage).toEqual({ stage: 'llm', done: 12, total: 40 });
    expect(s.stuck).toBe(false);
    expect(s.died).toBeNull();
  });

  it('этап save → stage очищается (прогон завершил этапы)', () => {
    useNewsStore.getState().onProgress({ stage: 'save', done: 1, total: 1 });
    expect(useNewsStore.getState().stage).toBeNull();
  });
});
