import { beforeEach, describe, expect, it } from 'vitest';
import { flattenVisible, useVaultStore } from './vault';

function reset() {
  useVaultStore.setState({
    info: null,
    childrenByPath: {},
    expanded: {},
    loading: {},
    selectedPath: null,
  });
}

beforeEach(reset);

describe('vault store (Ф0-3)', () => {
  it('openVault загружает корень', async () => {
    await useVaultStore.getState().openVault('');
    const s = useVaultStore.getState();
    expect(s.info).not.toBeNull();
    expect(s.childrenByPath['']?.length ?? 0).toBeGreaterThan(0);
  });

  it('toggleDir лениво грузит детей и раскрывает', async () => {
    await useVaultStore.getState().openVault('');
    await useVaultStore.getState().toggleDir('Projects');
    const s = useVaultStore.getState();
    expect(s.expanded['Projects']).toBe(true);
    expect(s.childrenByPath['Projects']).toBeDefined();
    const visible = flattenVisible(s.childrenByPath, s.expanded, s.loading);
    expect(visible.some((n) => n.entry.path === 'Projects/Roadmap.md')).toBe(true);
  });

  it('повторный toggleDir сворачивает, кэш детей сохраняется', async () => {
    const store = useVaultStore.getState();
    await store.openVault('');
    await store.toggleDir('Projects');
    await useVaultStore.getState().toggleDir('Projects');
    const s = useVaultStore.getState();
    expect(s.expanded['Projects']).toBeUndefined();
    expect(s.childrenByPath['Projects']).toBeDefined();
    const visible = flattenVisible(s.childrenByPath, s.expanded, s.loading);
    expect(visible.some((n) => n.entry.path.startsWith('Projects/'))).toBe(false);
  });

  it('flattenVisible отражает глубину вложенности', async () => {
    const store = useVaultStore.getState();
    await store.openVault('');
    await store.toggleDir('Projects');
    await useVaultStore.getState().toggleDir('Projects/Alpha');
    const s = useVaultStore.getState();
    const visible = flattenVisible(s.childrenByPath, s.expanded, s.loading);
    const spec = visible.find((n) => n.entry.path === 'Projects/Alpha/Spec.md');
    expect(spec?.depth).toBe(2);
  });
});
