import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../lib/tauri-api';
import { useNewsStore } from './news';

afterEach(() => {
  vi.restoreAllMocks();
  useNewsStore.setState({ items: [], topic: null, unreadOnly: false, loading: true, error: null });
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
