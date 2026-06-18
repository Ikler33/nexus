import { beforeEach, describe, expect, it } from 'vitest';

import * as mockEpisode from '../lib/mock/episode';
import { useEpisodeStore } from './episode';

// EP-3: стор панели эпизодов поверх мок-контракта. Проверяем КОНТРАКТ (зеркало бэка): dismiss/restore
// обратимы и строку не трогают; purge РЕАЛЬНО удаляет; setEnabled персистит.
describe('episode store (EP-3)', () => {
  beforeEach(() => {
    mockEpisode.__reset();
    useEpisodeStore.setState({ episodes: [], loading: false, enabled: false });
  });

  it('load — список в обратной хронологии (endedAt DESC)', async () => {
    await useEpisodeStore.getState().load();
    const eps = useEpisodeStore.getState().episodes;
    expect(eps.length).toBe(2);
    expect(eps[0].endedAt).toBeGreaterThanOrEqual(eps[1].endedAt);
  });

  it('dismiss скрывает (обратимо, строка жива), restore возвращает', async () => {
    await useEpisodeStore.getState().load();
    const id = useEpisodeStore.getState().episodes[0].id;
    await useEpisodeStore.getState().dismiss(id);
    expect(useEpisodeStore.getState().episodes.find((e) => e.id === id)?.dismissed).toBe(true);
    expect(useEpisodeStore.getState().episodes.length).toBe(2); // НЕ удалили строку
    await useEpisodeStore.getState().restore(id);
    expect(useEpisodeStore.getState().episodes.find((e) => e.id === id)?.dismissed).toBe(false);
  });

  it('purge РЕАЛЬНО удаляет строку (необратимо)', async () => {
    await useEpisodeStore.getState().load();
    const id = useEpisodeStore.getState().episodes[0].id;
    await useEpisodeStore.getState().purge(id);
    expect(useEpisodeStore.getState().episodes.find((e) => e.id === id)).toBeUndefined();
    expect(useEpisodeStore.getState().episodes.length).toBe(1);
  });

  it('setEnabled персистит, loadEnabled читает', async () => {
    await useEpisodeStore.getState().setEnabled(true);
    expect(useEpisodeStore.getState().enabled).toBe(true);
    useEpisodeStore.setState({ enabled: false });
    await useEpisodeStore.getState().loadEnabled();
    expect(useEpisodeStore.getState().enabled).toBe(true);
  });
});
