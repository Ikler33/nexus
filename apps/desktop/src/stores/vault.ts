import { create } from 'zustand';
import { tauriApi, type FileEntry, type NoteRef, type VaultInfo } from '../lib/tauri-api';
import { compareEntries } from '../i18n/format';

/** Узел плоского (развёрнутого) представления дерева для виртуализации. */
export interface FlatNode {
  entry: FileEntry;
  depth: number;
  expanded: boolean;
  loading: boolean;
}

interface VaultState {
  info: VaultInfo | null;
  /** Загруженные дети по пути каталога ('' = корень). Ленивая модель. */
  childrenByPath: Record<string, FileEntry[]>;
  expanded: Record<string, true>;
  loading: Record<string, true>;
  /** Все заметки vault (для автокомплита `[[wikilink]]` и резолва ссылок). */
  notes: NoteRef[];

  openVault: (path: string) => Promise<void>;
  toggleDir: (path: string) => Promise<void>;
}

export const useVaultStore = create<VaultState>((set, get) => ({
  info: null,
  childrenByPath: {},
  expanded: {},
  loading: {},
  notes: [],

  async openVault(path) {
    const info = await tauriApi.vault.openVault(path);
    const [root, notes] = await Promise.all([
      tauriApi.vault.listDir(''),
      tauriApi.vault.listNotes().catch(() => []),
    ]);
    set({
      info,
      childrenByPath: { '': [...root].sort(compareEntries) },
      expanded: {},
      loading: {},
      notes,
    });
  },

  async toggleDir(path) {
    const { expanded, childrenByPath } = get();
    if (expanded[path]) {
      const next = { ...expanded };
      delete next[path];
      set({ expanded: next });
      return;
    }
    if (childrenByPath[path]) {
      set((s) => ({ expanded: { ...s.expanded, [path]: true } }));
      return;
    }
    set((s) => ({ loading: { ...s.loading, [path]: true } }));
    try {
      const children = (await tauriApi.vault.listDir(path)).slice().sort(compareEntries);
      set((s) => {
        const loading = { ...s.loading };
        delete loading[path];
        return {
          childrenByPath: { ...s.childrenByPath, [path]: children },
          expanded: { ...s.expanded, [path]: true },
          loading,
        };
      });
    } catch (err) {
      set((s) => {
        const loading = { ...s.loading };
        delete loading[path];
        return { loading };
      });
      throw err;
    }
  },
}));

/** Имя заметки для wikilink (basename без `.md`). */
export function noteName(path: string): string {
  const base = path.slice(path.lastIndexOf('/') + 1);
  return base.endsWith('.md') ? base.slice(0, -3) : base;
}

/** Резолвит цель `[[wikilink]]` в путь файла среди известных заметок. */
export function resolveLink(target: string, notes: NoteRef[]): string | null {
  const want = target.endsWith('.md') ? target.slice(0, -3) : target;
  return (
    notes.find((n) => n.path === target)?.path ?? // точный путь
    notes.find((n) => n.path.replace(/\.md$/, '') === want)?.path ?? // путь без .md
    notes.find((n) => noteName(n.path) === noteName(want))?.path ?? // по имени файла
    null
  );
}

/** Плоский список ВИДИМЫХ узлов (только раскрытые ветви) для виртуализации. */
export function flattenVisible(
  childrenByPath: Record<string, FileEntry[]>,
  expanded: Record<string, true>,
  loading: Record<string, true>,
): FlatNode[] {
  const out: FlatNode[] = [];
  const walk = (path: string, depth: number) => {
    const children = childrenByPath[path];
    if (!children) return;
    for (const entry of children) {
      const isExpanded = !!expanded[entry.path];
      out.push({ entry, depth, expanded: isExpanded, loading: !!loading[entry.path] });
      if (entry.isDir && isExpanded) walk(entry.path, depth + 1);
    }
  };
  walk('', 0);
  return out;
}
