import { beforeEach, describe, expect, it, vi } from 'vitest';
import { tauriApi, type FileEntry } from '../lib/tauri-api';
import { flattenVisible, useVaultStore } from './vault';

const entry = (name: string): FileEntry => ({
  name,
  path: name,
  isDir: false,
  hasChildren: false,
  sizeBytes: 0,
});

function reset() {
  useVaultStore.setState({
    info: null,
    childrenByPath: {},
    expanded: {},
    loading: {},
  });
}

beforeEach(reset);

describe('vault store (Ф0-3/Ф0-9)', () => {
  it('openVault загружает корень (полный список заметок НЕ грузится, #22)', async () => {
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

  it('createNote: уникальное имя, пишет файл, обновляет дерево (кросс-план #1)', async () => {
    useVaultStore.setState({ childrenByPath: { '': [entry('Untitled.md')] } });
    const write = vi.spyOn(tauriApi.vault, 'writeFile').mockResolvedValue(undefined);
    vi.spyOn(tauriApi.vault, 'listDir').mockResolvedValue([
      entry('Untitled.md'),
      entry('Untitled 1.md'),
    ]);

    const path = await useVaultStore.getState().createNote('', { content: 'hi' });

    expect(path).toBe('Untitled 1.md'); // Untitled.md занят → Untitled 1.md
    expect(write).toHaveBeenCalledWith('Untitled 1.md', 'hi');
    expect(useVaultStore.getState().childrenByPath['']).toHaveLength(2);
    vi.restoreAllMocks();
  });
});

// #22: автокомплит/клик по ссылке спрашивают бэкенд; вне Tauri отвечает мок с той же семантикой.
describe('listNotes(query, limit) + resolveNote (#22, мок-зеркало бэкенда)', () => {
  it('listNotes фильтрует по подстроке и режет limit', async () => {
    const all = await tauriApi.vault.listNotes();
    expect(all.length).toBeGreaterThan(2);
    const road = await tauriApi.vault.listNotes('roadmap');
    expect(road.length).toBeGreaterThan(0);
    expect(road.every((n) => n.path.toLowerCase().includes('roadmap'))).toBe(true);
    const top1 = await tauriApi.vault.listNotes(undefined, 1);
    expect(top1).toHaveLength(1);
  });

  it('resolveNote: полный путь, путь без .md, basename; неизвестное → null', async () => {
    expect(await tauriApi.vault.resolveNote('Inbox.md')).toBe('Inbox.md');
    expect(await tauriApi.vault.resolveNote('Projects/Roadmap')).toBe('Projects/Roadmap.md');
    expect(await tauriApi.vault.resolveNote('Roadmap')).toBe('Projects/Roadmap.md');
    expect(await tauriApi.vault.resolveNote('Nonexistent')).toBeNull();
  });
});
