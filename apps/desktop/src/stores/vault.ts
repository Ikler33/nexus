import { create } from 'zustand';
import { VAULT_SWITCHED_EVENT } from '../lib/app-events';
import { tauriApi, type FileEntry, type VaultInfo } from '../lib/tauri-api';
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

  openVault: (path: string) => Promise<void>;
  toggleDir: (path: string) => Promise<void>;
  /**
   * Создаёт новую заметку в каталоге `dir` ('' = корень) с уникальным именем (`baseName`, по
   * умолчанию `Untitled`) и опциональным содержимым; пишет файл, обновляет дерево/notes и возвращает
   * путь. Используется командой `file.new`, кнопкой сайдбара и пустым состоянием дерева (кросс-план #1).
   */
  createNote: (dir?: string, opts?: { baseName?: string; content?: string }) => Promise<string>;
  /** Удаляет заметку/каталог в корзину (CURATE-1): закрывает открытые буферы пути и обновляет дерево. */
  deleteFile: (path: string) => Promise<void>;
  /** Переименовывает/перемещает путь (CURATE-2): сохраняет грязные буферы, переносит их и дерево. */
  renameFile: (from: string, to: string) => Promise<void>;
  /** Перечитать детей каталога ('' = корень) и опц. раскрыть его (после создания файла извне). */
  refreshDir: (dir: string, expand?: boolean) => Promise<void>;
  /** REVEAL-ACTIVE-FILE: раскрыть ВСЕ родительские каталоги пути (с догрузкой детей), чтобы файл стал
   *  видимым в дереве. Идемпотентно; персистит свёрнутость. Скролл к строке — на стороне FileTree. */
  revealPath: (path: string) => Promise<void>;
}

/** Родительский каталог пути ('' = корень). */
function parentDir(path: string): string {
  return path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
}

/**
 * Персист свёрнутости дерева между перезапусками (TREE-EXPANDED-PERSIST), по образцу starred.ts, но
 * с привязкой к vaultRoot: `{ [root]: string[] }` в localStorage. Иначе раскрытие одного vault протекло
 * бы в другой с тем же относительным путём. Несуществующие после внешних правок пути отсеиваются лениво
 * при загрузке (listDir упадёт → каталог пропускается).
 */
/** Монотонный токен открытия vault: защита от гонки re-entrant `openVault` (быстрое A→B). Поздняя
 *  continuation устаревшего открытия (после `await Promise.all` догрузки) НЕ затирает актуальный vault. */
let openSeq = 0;
const EXPANDED_KEY = 'nexus.tree-expanded.v1';
function readExpandedMap(): Record<string, string[]> {
  try {
    const raw = localStorage.getItem(EXPANDED_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, string[]>) : {};
  } catch {
    return {};
  }
}
function persistExpanded(root: string | null | undefined, expanded: Record<string, true>): void {
  if (!root) return;
  try {
    const map = readExpandedMap();
    const keys = Object.keys(expanded);
    if (keys.length) map[root] = keys;
    else delete map[root];
    localStorage.setItem(EXPANDED_KEY, JSON.stringify(map));
  } catch {
    /* ignore */
  }
}

export const useVaultStore = create<VaultState>((set, get) => ({
  info: null,
  childrenByPath: {},
  expanded: {},
  loading: {},

  // Полный список заметок НЕ грузится (#22): автокомплит `[[…` спрашивает топ-N по подстроке
  // (`listNotes(query, limit)`), клик по ссылке резолвит бэкенд (`resolveNote`) — payload открытия
  // vault не растёт с числом файлов.
  async openVault(path) {
    const token = ++openSeq; // ловим устаревшую continuation при быстром переключении vault
    const info = await tauriApi.vault.openVault(path);
    // Новый vault → кэши, ключованные путями заметок, недействительны (стартовые вопросы чата и т.п.).
    // Window-событие вместо прямого импорта components/chat (F-1: stores не знают компоненты) —
    // заинтересованная сторона подписывается сама; dispatchEvent синхронен, порядок прежний.
    window.dispatchEvent(new Event(VAULT_SWITCHED_EVENT));
    // Сбрасываем отклонённые предложения связей: ключ — относительный путь, в новом vault он чужой
    // (иначе dismiss «Notes/A.md» в vault A прячет связь в vault B с тем же путём — находка аудита).
    const { useSuggestStore } = await import('./suggest');
    useSuggestStore.getState().clearDismissed();
    const root = await tauriApi.vault.listDir('');
    const childrenByPath: Record<string, FileEntry[]> = { '': [...root].sort(compareEntries) };
    // TREE-EXPANDED-PERSIST: восстанавливаем свёрнутость прошлой сессии — грузим детей раскрытых
    // каталогов (иначе пометка expanded без children ничего не покажет). Каталог удалён снаружи →
    // listDir упадёт → пропускаем (заодно чистим устаревший путь из персиста на следующем сохранении).
    const expanded: Record<string, true> = {};
    const persisted = readExpandedMap()[info.root] ?? [];
    await Promise.all(
      persisted.map(async (p) => {
        try {
          childrenByPath[p] = (await tauriApi.vault.listDir(p)).slice().sort(compareEntries);
          expanded[p] = true;
        } catch {
          /* каталог исчез — не восстанавливаем */
        }
      }),
    );
    if (token !== openSeq) return; // более новый openVault уже выполнился — не затираем его дерево
    persistExpanded(info.root, expanded); // подчищаем отсеянные пути в сторадже
    set({ info, childrenByPath, expanded, loading: {} });
  },

  async toggleDir(path) {
    const { expanded, childrenByPath } = get();
    if (expanded[path]) {
      // Сворачивание каталога забывает и свёрнутость потомков — иначе в персисте остаётся `a/b`
      // без `a` (orphan: при рестарте грузится, но невидим; «b всё ещё открыт внутри a» — ревью).
      const next = { ...expanded };
      for (const k of Object.keys(next)) if (k === path || k.startsWith(`${path}/`)) delete next[k];
      set({ expanded: next });
      persistExpanded(get().info?.root, next);
      return;
    }
    if (childrenByPath[path]) {
      set((s) => ({ expanded: { ...s.expanded, [path]: true } }));
      persistExpanded(get().info?.root, get().expanded);
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
      persistExpanded(get().info?.root, get().expanded);
    } catch (err) {
      set((s) => {
        const loading = { ...s.loading };
        delete loading[path];
        return { loading };
      });
      throw err;
    }
  },

  async createNote(dir = '', opts = {}) {
    const base = opts.baseName ?? 'Untitled';
    const existing = new Set((get().childrenByPath[dir] ?? []).map((e) => e.name));
    let name = `${base}.md`;
    let i = 1;
    while (existing.has(name)) name = `${base} ${i++}.md`;
    const path = dir ? `${dir}/${name}` : name;
    await tauriApi.vault.writeFile(path, opts.content ?? '');
    // Обновляем детей каталога; раскрываем каталог. Автокомплит/резолв ссылок спрашивают бэкенд
    // на лету (#22) — отдельный список заметок поддерживать не нужно.
    const children = (await tauriApi.vault.listDir(dir)).slice().sort(compareEntries);
    set((s) => ({
      childrenByPath: { ...s.childrenByPath, [dir]: children },
      expanded: dir ? { ...s.expanded, [dir]: true } : s.expanded,
    }));
    if (dir) persistExpanded(get().info?.root, get().expanded); // авто-раскрытие каталога переживает рестарт
    return path;
  },

  async deleteFile(path) {
    await tauriApi.vault.deletePath(path);
    // Закрываем открытые буферы/вкладки удалённого пути (дин. импорт — без цикла vault↔workspace).
    const { useWorkspaceStore } = await import('./workspace');
    useWorkspaceStore.getState().dropPathsUnder(path);
    // P6-PIN: открепляем удалённый путь из контекста чата (не держим мёртвый пин).
    const { useChatStore } = await import('./chat');
    useChatStore.getState().dropPinsUnder(path);
    // Снимаем звёзды с удалённого пути и детей — иначе осиротевшие записи в Starred (находка аудита).
    const { useStarredStore } = await import('./starred');
    useStarredStore.getState().dropStarsUnder(path);
    // Обновляем детей родительского каталога (удалённый элемент исчезает из дерева) и выметаем
    // свёрнутость под удалённым путём (иначе осиротевшие записи в персисте — как у звёзд).
    const dir = parentDir(path);
    const children = (await tauriApi.vault.listDir(dir)).slice().sort(compareEntries);
    set((s) => {
      const expanded = { ...s.expanded };
      for (const k of Object.keys(expanded)) if (k === path || k.startsWith(`${path}/`)) delete expanded[k];
      return { childrenByPath: { ...s.childrenByPath, [dir]: children }, expanded };
    });
    persistExpanded(get().info?.root, get().expanded);
  },

  async refreshDir(dir, expand = false) {
    const children = (await tauriApi.vault.listDir(dir)).slice().sort(compareEntries);
    set((s) => ({
      childrenByPath: { ...s.childrenByPath, [dir]: children },
      expanded: expand && dir ? { ...s.expanded, [dir]: true } : s.expanded,
    }));
    if (expand && dir) persistExpanded(get().info?.root, get().expanded);
  },

  async revealPath(path) {
    const segs = path.split('/');
    segs.pop(); // имя файла — раскрываем только каталоги-предки
    if (segs.length === 0) return; // файл в корне — раскрывать нечего
    let acc = '';
    for (const seg of segs) {
      acc = acc ? `${acc}/${seg}` : seg;
      if (!get().childrenByPath[acc]) {
        try {
          const children = (await tauriApi.vault.listDir(acc)).slice().sort(compareEntries);
          set((s) => ({ childrenByPath: { ...s.childrenByPath, [acc]: children } }));
        } catch {
          return; // каталог недоступен (удалён снаружи) — дальше раскрывать бессмысленно
        }
      }
      if (!get().expanded[acc]) set((s) => ({ expanded: { ...s.expanded, [acc]: true } }));
    }
    persistExpanded(get().info?.root, get().expanded);
  },

  async renameFile(from, to) {
    if (from === to) return;
    const { useWorkspaceStore } = await import('./workspace');
    const { flush } = await import('./autosave');
    // Сохраняем грязные буферы под `from` ДО переноса (иначе автосейв на старом пути воскресит файл).
    const ws = useWorkspaceStore.getState();
    for (const p of Object.keys(ws.buffers)) {
      if (p === from || p.startsWith(`${from}/`)) await flush(p);
    }
    await tauriApi.vault.renamePath(from, to);
    useWorkspaceStore.getState().renameBufferPath(from, to);
    // P6-PIN: переписываем закреплённые пути (иначе после rename на старый путь может лечь чужая
    // заметка → неверный контекст ИИ).
    const { useChatStore } = await import('./chat');
    useChatStore.getState().renamePins(from, to);
    // Переносим звёзды (точный путь + дети) — иначе звезда осиротевает на старом пути (находка аудита).
    const { useStarredStore } = await import('./starred');
    useStarredStore.getState().rename(from, to);
    // Обновляем оба затронутых каталога (источник и приёмник могут отличаться при move).
    const dirs = Array.from(new Set([parentDir(from), parentDir(to)]));
    for (const d of dirs) {
      const children = (await tauriApi.vault.listDir(d)).slice().sort(compareEntries);
      set((s) => ({ childrenByPath: { ...s.childrenByPath, [d]: children } }));
    }
    // Выметаем свёрнутость под старым путём: перенесённый каталог сворачивается (как и было до персиста —
    // childrenByPath поддерева не ремапится), но устаревшие ключи не оседают в сторадже.
    set((s) => {
      const expanded = { ...s.expanded };
      for (const k of Object.keys(expanded)) if (k === from || k.startsWith(`${from}/`)) delete expanded[k];
      return { expanded };
    });
    persistExpanded(get().info?.root, get().expanded);
  },
}));

/** Имя заметки для wikilink (basename без `.md`). */
export function noteName(path: string): string {
  const base = path.slice(path.lastIndexOf('/') + 1);
  return base.endsWith('.md') ? base.slice(0, -3) : base;
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
