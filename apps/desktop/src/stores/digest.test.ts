import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../lib/tauri-api';
import { useDigestStore } from './digest';

// Эталонный дайджест (createdAt = baseline).
const DIGEST_BASE = { createdAt: 1_000, since: 900, content: 'baseline', noteCount: 5 };

afterEach(() => {
  vi.restoreAllMocks();
  useDigestStore.setState({
    latest: null,
    loading: false,
    generating: false,
    error: null,
    baseline: null,
  });
});

// ── NB-4: is_kind_busy-футган — digest является recurring-kind (раз/сутки). После КАЖДОГО прогона
// воркер немедленно ставит следующий pending «на завтра» (reschedule_if_absent). Поэтому старый
// `jobActive('digest')` (Rust is_kind_busy) в steady state возвращал true → «Генерирую…» не гасло
// при сбое. Фикс: isJobReady (ready-семантика) — только running/pending с наступившим run_at. ────
describe('digest store — is_kind_busy-футган (NB-4)', () => {
  it('КРИТИЧЕСКИЙ: steady state — только «завтрашняя» recurring-pending → generating сбрасывается', async () => {
    // Сценарий: прогон завершился с ошибкой (нет нового дайджеста), воркер немедленно
    // переназначил следующий pending на завтра. До NB-4 кнопка «Генерировать» висела вечно.
    useDigestStore.setState({ generating: true, baseline: DIGEST_BASE.createdAt });
    vi.spyOn(tauriApi.digest, 'latest').mockResolvedValue(DIGEST_BASE); // тот же createdAt → gotNew=false
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      {
        id: 7,
        kind: 'digest',
        state: 'pending',
        runAt: Math.floor(Date.now() / 1000) + 86_400, // «завтрашняя» recurring
        attempts: 0,
      },
    ]);

    await useDigestStore.getState().load();

    // До фикса: jobActive('digest') = true (pending есть) → stillGenerating = true → вечный спиннер.
    // После фикса: isJobReady('digest', ...) = false (pending в будущем) → stillGenerating = false.
    expect(useDigestStore.getState().generating).toBe(false);
  });

  it('running-джоба (прогон идёт) → generating остаётся true', async () => {
    useDigestStore.setState({ generating: true, baseline: DIGEST_BASE.createdAt });
    vi.spyOn(tauriApi.digest, 'latest').mockResolvedValue(DIGEST_BASE); // gotNew=false
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      {
        id: 42,
        kind: 'digest',
        state: 'running',
        runAt: Math.floor(Date.now() / 1000),
        attempts: 1,
      },
    ]);

    await useDigestStore.getState().load();

    expect(useDigestStore.getState().generating).toBe(true);
  });

  it('новый дайджест (gotNew) → generating сбрасывается независимо от очереди', async () => {
    // Успешный прогон: createdAt изменился. Кнопка должна погаснуть даже если running-джоба в очереди.
    useDigestStore.setState({ generating: true, baseline: DIGEST_BASE.createdAt });
    vi.spyOn(tauriApi.digest, 'latest').mockResolvedValue({
      ...DIGEST_BASE,
      createdAt: 2_000, // новый createdAt → gotNew=true
    });
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      { id: 42, kind: 'digest', state: 'running', runAt: Math.floor(Date.now() / 1000), attempts: 1 },
    ]);

    await useDigestStore.getState().load();

    expect(useDigestStore.getState().generating).toBe(false);
    expect(useDigestStore.getState().latest?.createdAt).toBe(2_000);
  });

  it('generating=false → activeJobs не вызывается (оптимизация: незаинтересованные load-ы)', async () => {
    useDigestStore.setState({ generating: false, baseline: null });
    vi.spyOn(tauriApi.digest, 'latest').mockResolvedValue(DIGEST_BASE);
    const spyActiveJobs = vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([]);

    await useDigestStore.getState().load();

    // activeJobs не нужен, когда generating=false — не должен лишний раз ходить на бэкенд.
    expect(spyActiveJobs).not.toHaveBeenCalled();
    expect(useDigestStore.getState().generating).toBe(false);
  });

  it('pending с наступившим run_at (вот-вот стартует) → generating остаётся true', async () => {
    useDigestStore.setState({ generating: true, baseline: DIGEST_BASE.createdAt });
    vi.spyOn(tauriApi.digest, 'latest').mockResolvedValue(DIGEST_BASE); // gotNew=false
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      {
        id: 43,
        kind: 'digest',
        state: 'pending',
        runAt: Math.floor(Date.now() / 1000) - 1, // run_at уже наступил → isJobReady=true
        attempts: 0,
      },
    ]);

    await useDigestStore.getState().load();

    expect(useDigestStore.getState().generating).toBe(true);
  });
});
