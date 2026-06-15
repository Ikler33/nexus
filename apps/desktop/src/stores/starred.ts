import { create } from 'zustand';

/**
 * «Избранное» (DP-2, панель Starred сайдбара): набор путей заметок со звёздочкой.
 * v1 — localStorage на vault не завязан (пути относительные, при смене vault несуществующие
 * просто не покажутся); переезд в `.nexus`/БД — по спросу (BACKLOG, синк между устройствами).
 */
const KEY = 'nexus.starred.v1';

function read(): string[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((p): p is string => typeof p === 'string') : [];
  } catch {
    return [];
  }
}

function persist(paths: string[]): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(paths));
  } catch {
    /* ignore */
  }
}

interface StarredState {
  /** Пути избранных заметок (порядок добавления). */
  paths: string[];
  isStarred: (path: string) => boolean;
  toggle: (path: string) => void;
  /** Перенос путей при rename/move заметки ИЛИ каталога (точный путь + дети под `from/`). */
  rename: (from: string, to: string) => void;
  /** Снять звёзды с удалённого пути и всех его детей (заметка/каталог в корзину). */
  dropStarsUnder: (path: string) => void;
}

export const useStarredStore = create<StarredState>((set, get) => ({
  paths: read(),
  isStarred: (path) => get().paths.includes(path),
  toggle: (path) =>
    set((s) => {
      const paths = s.paths.includes(path)
        ? s.paths.filter((p) => p !== path)
        : [...s.paths, path];
      persist(paths);
      return { paths };
    }),
  rename: (from, to) =>
    set((s) => {
      const paths = s.paths.map((p) =>
        p === from ? to : p.startsWith(`${from}/`) ? `${to}/${p.slice(from.length + 1)}` : p,
      );
      persist(paths);
      return { paths };
    }),
  dropStarsUnder: (path) =>
    set((s) => {
      const paths = s.paths.filter((p) => p !== path && !p.startsWith(`${path}/`));
      persist(paths);
      return { paths };
    }),
}));
