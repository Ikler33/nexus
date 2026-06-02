import { beforeEach, describe, expect, it } from 'vitest';
import type { NoteRef } from '../lib/tauri-api';
import { flattenVisible, resolveLink, useVaultStore } from './vault';

function reset() {
  useVaultStore.setState({
    info: null,
    childrenByPath: {},
    expanded: {},
    loading: {},
    notes: [],
  });
}

beforeEach(reset);

describe('vault store (Ф0-3/Ф0-9)', () => {
  it('openVault загружает корень и заметки', async () => {
    await useVaultStore.getState().openVault('');
    const s = useVaultStore.getState();
    expect(s.info).not.toBeNull();
    expect(s.childrenByPath['']?.length ?? 0).toBeGreaterThan(0);
    expect(s.notes.length).toBeGreaterThan(0);
  });

  it('toggleDir лениво грузит детей и раскрывает', async () => {
    await useVaultStore.getState().openVault('');
    await useVaultStore.getState().toggleDir('Projects');
    const s = useVaultStore.getState();
    expect(s.expanded['Projects']).toBe(true);
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
  });

  it('flattenVisible отражает глубину вложенности', async () => {
    const store = useVaultStore.getState();
    await store.openVault('');
    await store.toggleDir('Projects');
    await useVaultStore.getState().toggleDir('Projects/Alpha');
    const s = useVaultStore.getState();
    const visible = flattenVisible(s.childrenByPath, s.expanded, s.loading);
    expect(visible.find((n) => n.entry.path === 'Projects/Alpha/Spec.md')?.depth).toBe(2);
  });
});

describe('resolveLink (Ф0-5)', () => {
  const notes: NoteRef[] = [
    { path: 'Inbox.md', title: null },
    { path: 'Projects/Roadmap.md', title: null },
    { path: 'Notes/Meeting.md', title: 'Weekly' },
  ];

  it('резолвит по полному пути, пути без .md и по имени', () => {
    expect(resolveLink('Inbox.md', notes)).toBe('Inbox.md');
    expect(resolveLink('Projects/Roadmap', notes)).toBe('Projects/Roadmap.md');
    expect(resolveLink('Meeting', notes)).toBe('Notes/Meeting.md');
    expect(resolveLink('Nonexistent', notes)).toBeNull();
  });
});
