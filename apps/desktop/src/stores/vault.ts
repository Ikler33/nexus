import { create } from 'zustand';
import { tauriApi, type FileEntry, type VaultInfo } from '../lib/tauri-api';

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
  /** Раскрытые каталоги. */
  expanded: Record<string, true>;
  /** Каталоги в процессе загрузки детей. */
  loading: Record<string, true>;
  selectedPath: string | null;

  openVault: (path: string) => Promise<void>;
  toggleDir: (path: string) => Promise<void>;
  selectFile: (path: string) => void;
}

export const useVaultStore = create<VaultState>((set, get) => ({
  info: null,
  childrenByPath: {},
  expanded: {},
  loading: {},
  selectedPath: null,

  async openVault(path) {
    const info = await tauriApi.vault.openVault(path);
    const root = await tauriApi.vault.listDir('');
    set({
      info,
      childrenByPath: { '': root },
      expanded: {},
      loading: {},
      selectedPath: null,
    });
  },

  async toggleDir(path) {
    const { expanded, childrenByPath } = get();

    // Свернуть (дети остаются в кэше — повторное раскрытие мгновенно).
    if (expanded[path]) {
      const next = { ...expanded };
      delete next[path];
      set({ expanded: next });
      return;
    }

    // Раскрыть: при необходимости лениво подгрузить детей.
    if (childrenByPath[path]) {
      set((s) => ({ expanded: { ...s.expanded, [path]: true } }));
      return;
    }

    set((s) => ({ loading: { ...s.loading, [path]: true } }));
    try {
      const children = await tauriApi.vault.listDir(path);
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

  selectFile(path) {
    set({ selectedPath: path });
  },
}));

/**
 * Плоский список ВИДИМЫХ узлов (только раскрытые ветви) для виртуализации.
 * Чистая функция от срезов стора — мемоизируется в компоненте по ссылкам на эти срезы.
 */
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
      out.push({
        entry,
        depth,
        expanded: isExpanded,
        loading: !!loading[entry.path],
      });
      if (entry.isDir && isExpanded) walk(entry.path, depth + 1);
    }
  };
  walk('', 0);
  return out;
}
