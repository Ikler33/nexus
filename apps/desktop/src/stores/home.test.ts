import { afterEach, describe, expect, it, vi } from 'vitest';

import { useHomeStore } from './home';
import { tauriApi } from '../lib/tauri-api';

afterEach(() => vi.restoreAllMocks());

describe('home store: syncGenerating (AIP-5 — честный «генерирю…»)', () => {
  it('активная home_widget:-джоба → generating[ключ]=true', async () => {
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      { id: 1, kind: 'home_widget:open_questions', state: 'running', runAt: 0, attempts: 1 },
      { id: 2, kind: 'home_widget:context_drift', state: 'pending', runAt: 0, attempts: 0 },
      { id: 3, kind: 'newsfeed', state: 'pending', runAt: 0, attempts: 0 },
    ]);
    useHomeStore.setState({ generating: {} });
    await useHomeStore.getState().syncGenerating();
    const g = useHomeStore.getState().generating;
    expect(g.open_questions).toBe(true);
    expect(g.context_drift).toBe(true);
    expect(g.newsfeed).toBeUndefined(); // не home_widget: — не трогаем
  });

  it('только ДОБАВЛЯЕТ флаги (снятие — по widget-updated), не сбрасывает чужие', async () => {
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([]);
    useHomeStore.setState({ generating: { context_drift: true } });
    await useHomeStore.getState().syncGenerating();
    expect(useHomeStore.getState().generating.context_drift).toBe(true); // не снят, хотя джоб нет
  });

  it('ошибка activeJobs (нет планировщика) → no-op без краша', async () => {
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockRejectedValue(new Error('no scheduler'));
    useHomeStore.setState({ generating: {} });
    await expect(useHomeStore.getState().syncGenerating()).resolves.toBeUndefined();
    expect(useHomeStore.getState().generating).toEqual({});
  });
});
