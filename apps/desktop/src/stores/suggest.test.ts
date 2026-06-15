import { beforeEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../lib/tauri-api';
import { useSuggestStore } from './suggest';
import { useWorkspaceStore } from './workspace';

const SUG = [
  { path: 'B.md', title: null, score: 0.9, reason: 'причина B' },
  { path: 'C.md', title: null, score: 0.7, reason: 'причина C' },
];

beforeEach(() => {
  useSuggestStore.setState({ path: null, items: [], loading: false });
  vi.restoreAllMocks();
});

describe('suggest store (Ф1-9)', () => {
  it('load заполняет items для пути', async () => {
    vi.spyOn(tauriApi.suggest, 'forFile').mockResolvedValue(SUG);
    await useSuggestStore.getState().load('load.md');
    expect(useSuggestStore.getState().items.map((i) => i.path)).toEqual(['B.md', 'C.md']);
    expect(useSuggestStore.getState().loading).toBe(false);
  });

  it('null path → пусто', async () => {
    await useSuggestStore.getState().load(null);
    expect(useSuggestStore.getState().items).toEqual([]);
  });

  it('dismiss убирает и не возвращает при пересчёте', async () => {
    vi.spyOn(tauriApi.suggest, 'forFile').mockResolvedValue(SUG);
    await useSuggestStore.getState().load('dis.md');
    useSuggestStore.getState().dismiss('B.md');
    expect(useSuggestStore.getState().items.map((i) => i.path)).toEqual(['C.md']);
    await useSuggestStore.getState().load('dis.md'); // пересчёт
    expect(useSuggestStore.getState().items.map((i) => i.path)).toEqual(['C.md']);
  });

  // audit B8: смена vault обязана сбросить отклонённые цели — ключ dismissed это относительный
  // путь, в другом vault он чужой (иначе dismiss скрыл бы связь в новом vault с тем же путём).
  it('clearDismissed возвращает ранее отклонённые цели при пересчёте', async () => {
    vi.spyOn(tauriApi.suggest, 'forFile').mockResolvedValue(SUG);
    await useSuggestStore.getState().load('clr.md');
    useSuggestStore.getState().dismiss('B.md');
    await useSuggestStore.getState().load('clr.md');
    expect(useSuggestStore.getState().items.map((i) => i.path)).toEqual(['C.md']); // всё ещё скрыт

    useSuggestStore.getState().clearDismissed();
    await useSuggestStore.getState().load('clr.md');
    expect(useSuggestStore.getState().items.map((i) => i.path)).toEqual(['B.md', 'C.md']); // вернулся
  });

  it('accept дописывает [[wikilink]] в активный буфер и убирает из списка', async () => {
    vi.spyOn(tauriApi.suggest, 'forFile').mockResolvedValue(SUG);
    useWorkspaceStore.setState({
      buffers: { 'acc.md': { path: 'acc.md', doc: '# A', dirty: false, baseHash: '' } },
      groups: [{ id: 'g0', tabs: ['acc.md'], activeTab: 'acc.md' }],
      activeGroupId: 'g0',
    });
    await useSuggestStore.getState().load('acc.md');
    useSuggestStore.getState().accept('B.md');

    const buf = useWorkspaceStore.getState().buffers['acc.md'];
    expect(buf.doc).toContain('[[B]]');
    expect(buf.dirty).toBe(true);
    expect(useSuggestStore.getState().items.map((i) => i.path)).toEqual(['C.md']);
  });
});
