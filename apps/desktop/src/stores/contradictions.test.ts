import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../lib/tauri-api';
import { useContradictionsStore } from './contradictions';

/** Эталонный набор противоречий (createdAt = baseline). */
const CONTRA_BASE = [
  { pathA: 'a.md', pathB: 'b.md', ctype: 'hard', explanation: 'противоречие', createdAt: 1_000 },
];
/** Пустой набор (нет противоречий) с тем же baseline-stamp = null. */
const CONTRA_EMPTY: never[] = [];

afterEach(() => {
  vi.restoreAllMocks();
  useContradictionsStore.setState({
    items: [],
    loading: false,
    generating: false,
    error: null,
    baseline: null,
  });
});

// ── NB-4: is_kind_busy-футган — contradictions является recurring-kind (раз/сутки). После КАЖДОГО
// прогона воркер немедленно ставит следующий pending «на завтра» (reschedule_if_absent). Поэтому
// старый `jobActive('contradictions')` (Rust is_kind_busy) в steady state возвращал true →
// «Ищу…» не гасло при сбое. Фикс: isJobReady (ready-семантика) — только running/pending с наступившим run_at.
describe('contradictions store — is_kind_busy-футган (NB-4)', () => {
  it('КРИТИЧЕСКИЙ: steady state — только «завтрашняя» recurring-pending → generating сбрасывается', async () => {
    // Сценарий: прогон завершился с ошибкой (нет нового набора), воркер переназначил на завтра.
    // До NB-4 кнопка «Найти противоречия» висела в состоянии «Ищу…» вечно.
    useContradictionsStore.setState({ generating: true, baseline: CONTRA_BASE[0].createdAt, items: CONTRA_BASE });
    vi.spyOn(tauriApi.contradictions, 'list').mockResolvedValue(CONTRA_BASE); // тот же stamp → gotNew=false
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      {
        id: 7,
        kind: 'contradictions',
        state: 'pending',
        runAt: Math.floor(Date.now() / 1000) + 86_400, // «завтрашняя» recurring
        attempts: 0,
      },
    ]);

    await useContradictionsStore.getState().load();

    // До фикса: jobActive('contradictions') = true → stillGenerating = true → вечный спиннер.
    // После фикса: isJobReady = false (pending в будущем) → stillGenerating = false.
    expect(useContradictionsStore.getState().generating).toBe(false);
  });

  it('running-джоба (прогон идёт) → generating остаётся true', async () => {
    useContradictionsStore.setState({ generating: true, baseline: CONTRA_BASE[0].createdAt, items: CONTRA_BASE });
    vi.spyOn(tauriApi.contradictions, 'list').mockResolvedValue(CONTRA_BASE); // gotNew=false
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      {
        id: 42,
        kind: 'contradictions',
        state: 'running',
        runAt: Math.floor(Date.now() / 1000),
        attempts: 1,
      },
    ]);

    await useContradictionsStore.getState().load();

    expect(useContradictionsStore.getState().generating).toBe(true);
  });

  it('новый набор противоречий (gotNew) → generating сбрасывается независимо от очереди', async () => {
    // Успешный прогон: createdAt первого элемента изменился. Кнопка должна погаснуть.
    useContradictionsStore.setState({ generating: true, baseline: CONTRA_BASE[0].createdAt, items: CONTRA_BASE });
    vi.spyOn(tauriApi.contradictions, 'list').mockResolvedValue([
      { ...CONTRA_BASE[0], createdAt: 2_000 }, // новый stamp → gotNew=true
    ]);
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      { id: 42, kind: 'contradictions', state: 'running', runAt: Math.floor(Date.now() / 1000), attempts: 1 },
    ]);

    await useContradictionsStore.getState().load();

    expect(useContradictionsStore.getState().generating).toBe(false);
  });

  it('baseline=null, пустой список → stamp совпадает (null===null), проверяем isJobReady', async () => {
    // Прогресс без противоречий: список пуст до и после → gotNew=false.
    // Проверяем, что «завтрашняя» pending всё равно сбрасывает generating.
    useContradictionsStore.setState({ generating: true, baseline: null, items: CONTRA_EMPTY });
    vi.spyOn(tauriApi.contradictions, 'list').mockResolvedValue(CONTRA_EMPTY); // stamp null===null → gotNew=false
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      {
        id: 9,
        kind: 'contradictions',
        state: 'pending',
        runAt: Math.floor(Date.now() / 1000) + 86_400,
        attempts: 0,
      },
    ]);

    await useContradictionsStore.getState().load();

    expect(useContradictionsStore.getState().generating).toBe(false);
  });

  it('generating=false → activeJobs не вызывается (оптимизация)', async () => {
    useContradictionsStore.setState({ generating: false, baseline: null });
    vi.spyOn(tauriApi.contradictions, 'list').mockResolvedValue([]);
    const spyActiveJobs = vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([]);

    await useContradictionsStore.getState().load();

    expect(spyActiveJobs).not.toHaveBeenCalled();
    expect(useContradictionsStore.getState().generating).toBe(false);
  });

  it('pending с наступившим run_at → generating остаётся true', async () => {
    useContradictionsStore.setState({ generating: true, baseline: CONTRA_BASE[0].createdAt, items: CONTRA_BASE });
    vi.spyOn(tauriApi.contradictions, 'list').mockResolvedValue(CONTRA_BASE); // gotNew=false
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      {
        id: 43,
        kind: 'contradictions',
        state: 'pending',
        runAt: Math.floor(Date.now() / 1000) - 1, // уже наступил → isJobReady=true
        attempts: 0,
      },
    ]);

    await useContradictionsStore.getState().load();

    expect(useContradictionsStore.getState().generating).toBe(true);
  });
});
