import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../lib/tauri-api';
import { useRelatedStore, visibleRelated } from './related';
import { useWorkspaceStore } from './workspace';

afterEach(() => {
  vi.restoreAllMocks();
  useRelatedStore.setState({ path: null, items: [], loading: false });
});

describe('related store (#35)', () => {
  it('load: получает похожие, порог фильтрует видимые', async () => {
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([
      { path: 'B.md', title: 'B', score: 0.8, reason: 'r' },
      { path: 'C.md', title: 'C', score: 0.3, reason: 'r' },
    ]);
    await useRelatedStore.getState().load('A.md');
    const s = useRelatedStore.getState();
    expect(s.items).toHaveLength(2);
    expect(visibleRelated(s.items, 0)).toHaveLength(2); // порог 0 → все
    expect(visibleRelated(s.items, 0.5).map((i) => i.path)).toEqual(['B.md']); // 50% → только B
  });

  it('insertLink: дописывает [[wikilink]] в буфер и НЕ убирает строку (AC-RN-6)', async () => {
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([
      { path: 'B.md', title: 'B', score: 0.9, reason: 'r' },
    ]);
    useWorkspaceStore.setState({
      buffers: { 'A.md': { path: 'A.md', doc: 'текст', dirty: false } },
      groups: [{ id: 'g0', tabs: ['A.md'], activeTab: 'A.md' }],
      activeGroupId: 'g0',
    });
    await useRelatedStore.getState().load('A.md');
    useRelatedStore.getState().insertLink('B.md');

    const buf = useWorkspaceStore.getState().buffers['A.md'];
    expect(buf.doc).toContain('[[B]]');
    expect(buf.dirty).toBe(true);
    expect(useRelatedStore.getState().items).toHaveLength(1); // строка осталась (дискавери)
  });

  it('setThreshold: клампится в [0,1]', () => {
    useRelatedStore.getState().setThreshold(1.5);
    expect(useRelatedStore.getState().threshold).toBe(1);
    useRelatedStore.getState().setThreshold(-0.2);
    expect(useRelatedStore.getState().threshold).toBe(0);
  });
});
