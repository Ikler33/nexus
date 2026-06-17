import { beforeEach, describe, expect, it, vi } from 'vitest';
import { tauriApi, type FileEntry } from '../lib/tauri-api';
import { flattenVisible, useVaultStore } from './vault';

// node-тестовый localStorage не функционирует (грабля node25) — подменяем Map-стабом, иначе персист
// свёрнутости (TREE-EXPANDED-PERSIST) молча не пишется и тесты round-trip недостоверны.
const lsStore = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => lsStore.get(k) ?? null,
  setItem: (k: string, v: string) => void lsStore.set(k, v),
  removeItem: (k: string) => void lsStore.delete(k),
  clear: () => lsStore.clear(),
});

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
  // openVault теперь читает свёрнутость из localStorage (TREE-EXPANDED-PERSIST) → изолируем тесты.
  try {
    localStorage.clear();
  } catch {
    /* ignore */
  }
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

  it('TREE-EXPANDED-PERSIST: раскрытие переживает «перезапуск» (localStorage по vaultRoot)', async () => {
    await useVaultStore.getState().openVault('');
    await useVaultStore.getState().toggleDir('Projects');
    expect(useVaultStore.getState().expanded['Projects']).toBe(true);
    // «Перезапуск»: сбрасываем in-memory (НЕ localStorage) и снова открываем тот же vault.
    useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
    await useVaultStore.getState().openVault('');
    const s = useVaultStore.getState();
    expect(s.expanded['Projects']).toBe(true); // восстановлено
    expect(s.childrenByPath['Projects']).toBeDefined(); // дети раскрытого каталога загружены заранее
  });

  it('TREE-EXPANDED-PERSIST: исчезнувший каталог отсеивается при загрузке', async () => {
    await useVaultStore.getState().openVault('');
    const root = useVaultStore.getState().info?.root ?? '';
    // Кладём в персист несуществующий путь; listDir по нему упадёт.
    localStorage.setItem('nexus.tree-expanded.v1', JSON.stringify({ [root]: ['NoSuchDir'] }));
    const orig = tauriApi.vault.listDir;
    vi.spyOn(tauriApi.vault, 'listDir').mockImplementation((dir: string) =>
      dir === 'NoSuchDir' ? Promise.reject(new Error('gone')) : orig(dir),
    );
    useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
    await useVaultStore.getState().openVault('');
    expect(useVaultStore.getState().expanded['NoSuchDir']).toBeUndefined(); // отсеяно
    vi.restoreAllMocks();
  });

  it('TREE-EXPANDED-PERSIST: сворачивание родителя забывает потомков (нет orphan в персисте)', async () => {
    await useVaultStore.getState().openVault('');
    await useVaultStore.getState().toggleDir('Projects');
    await useVaultStore.getState().toggleDir('Projects/Alpha');
    expect(useVaultStore.getState().expanded['Projects/Alpha']).toBe(true);
    await useVaultStore.getState().toggleDir('Projects'); // сворачиваем родителя
    const s = useVaultStore.getState();
    expect(s.expanded['Projects']).toBeUndefined();
    expect(s.expanded['Projects/Alpha']).toBeUndefined(); // потомок тоже забыт
    const root = s.info?.root ?? '';
    const persisted = (JSON.parse(localStorage.getItem('nexus.tree-expanded.v1') ?? '{}') as Record<string, string[]>)[root] ?? [];
    expect(persisted).not.toContain('Projects/Alpha');
  });

  it('TREE-EXPANDED-PERSIST: гонка openVault — поздний старый vault не затирает новый (epoch-guard)', async () => {
    const realOpen = tauriApi.vault.openVault.bind(tauriApi.vault);
    vi.spyOn(tauriApi.vault, 'openVault').mockImplementation(async (p: string) => {
      if (p === 'A') await new Promise((r) => setTimeout(r, 20)); // vault A резолвится МЕДЛЕННЕЕ
      return realOpen(p);
    });
    const pA = useVaultStore.getState().openVault('A');
    const pB = useVaultStore.getState().openVault('B');
    await Promise.all([pA, pB]);
    expect(useVaultStore.getState().info?.root).toBe('B'); // B выиграл, continuation A отбита токеном
    vi.restoreAllMocks();
  });

  it('createNote: уникальное имя, пишет файл, обновляет дерево (кросс-план #1)', async () => {
    useVaultStore.setState({ childrenByPath: { '': [entry('Untitled.md')] } });
    const write = vi.spyOn(tauriApi.vault, 'writeFile').mockResolvedValue('hash');
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
