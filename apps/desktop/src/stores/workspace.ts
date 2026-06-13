import { create } from 'zustand';
import { isViewable } from '../lib/file-kind';
import { tauriApi } from '../lib/tauri-api';

/**
 * Рабочее пространство (§4.1, Б12): группы (сплиты) и вкладки вместо одиночного `currentFile`.
 * Активный документ = активная вкладка активной группы — на него завязаны AI-контекст,
 * backlinks, suggest. Буфер — один на путь (общий между группами); правки сохраняют `dirty`
 * при переключении вкладок (AC-Б12-2).
 */

/** Открытый буфер документа (источник истины содержимого до сохранения). */
export interface Buffer {
  path: string;
  doc: string;
  dirty: boolean;
  /** Хеш контента на момент последней синхронизации с диском (open / save / accept-загрузка).
   *  Эхо своего сейва не поднимает guard внешнего изменения; расхождение с диском = внешняя правка
   *  (SAFE-2/3). Для бинарных (картинка/PDF) буферов — пустая строка. */
  baseHash: string;
}

/** Группа (сплит): набор вкладок + активная вкладка. */
export interface EditorGroup {
  id: string;
  tabs: string[]; // пути буферов
  activeTab: string | null;
}

const INITIAL_GROUP = 'g0';
let groupSeq = 0;
const nextGroupId = () => `g${++groupSeq}`;

interface WorkspaceState {
  buffers: Record<string, Buffer>;
  groups: EditorGroup[];
  activeGroupId: string;

  openFile: (path: string, groupId?: string) => Promise<void>;
  openLink: (target: string) => Promise<void>;
  setActiveTab: (groupId: string, path: string) => void;
  setActiveGroup: (groupId: string) => void;
  closeTab: (groupId: string, path: string) => void;
  /** DnD вкладок между панами (DP-3, контракт `text/nexus-tab` макета): перенос без дублей,
   *  опустевшая группа схлопывается, буфер жив (в отличие от closeTab — без GC). */
  moveTab: (fromGroupId: string, toGroupId: string, path: string) => void;
  /** Режим source/preview АКТИВНОЙ группы (DP-3 mode-float, ⌘E). */
  modes: Record<string, 'source' | 'preview'>;
  toggleMode: (groupId?: string) => void;
  splitRight: () => void;
  updateBufferDoc: (path: string, doc: string) => void;
  saveBuffer: (path: string) => Promise<void>;
  reset: () => void;
}

function freshGroups(): EditorGroup[] {
  return [{ id: INITIAL_GROUP, tabs: [], activeTab: null }];
}

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  buffers: {},
  groups: freshGroups(),
  activeGroupId: INITIAL_GROUP,

  async openFile(path, groupId) {
    // Открытие файла переключает main-область на редактор (Home/News — полные вьюхи, DP-1).
    const { useUIStore } = await import('./ui');
    useUIStore.getState().closeHome();
    useUIStore.getState().closeNews();
    const gid = groupId ?? get().activeGroupId;
    let buffers = get().buffers;
    if (!buffers[path]) {
      // Текстовая заметка: контент + baseHash (отпечаток диска для guard внешних изменений, SAFE-3).
      // Бинарь (картинка/PDF) не читаем как текст — его покажет FileViewer (asset-URL), baseHash пуст.
      let doc = '';
      let baseHash = '';
      if (!isViewable(path)) {
        const meta = await tauriApi.vault.readFileMeta(path);
        doc = meta.content;
        baseHash = meta.hash;
      }
      buffers = { ...buffers, [path]: { path, doc, dirty: false, baseHash } };
    }
    set((s) => ({
      buffers,
      activeGroupId: gid,
      groups: s.groups.map((g) =>
        g.id === gid
          ? {
              ...g,
              tabs: g.tabs.includes(path) ? g.tabs : [...g.tabs, path],
              activeTab: path,
            }
          : g,
      ),
    }));
  },

  async openLink(target) {
    // Резолв на бэкенде (#22) — та же семантика, что у индексатора links (путь/±.md/basename,
    // затем алиас V4.1): фронт не держит полный список заметок, алиасные ссылки кликабельны.
    const path = await tauriApi.vault.resolveNote(target).catch(() => null);
    if (path) await get().openFile(path);
  },

  setActiveTab(groupId, path) {
    set((s) => ({
      activeGroupId: groupId,
      groups: s.groups.map((g) => (g.id === groupId ? { ...g, activeTab: path } : g)),
    }));
  },

  setActiveGroup(groupId) {
    set({ activeGroupId: groupId });
  },

  closeTab(groupId, path) {
    set((s) => {
      const updated = s.groups.map((g) => {
        if (g.id !== groupId) return g;
        const tabs = g.tabs.filter((t) => t !== path);
        const activeTab = g.activeTab === path ? (tabs[tabs.length - 1] ?? null) : g.activeTab;
        return { ...g, tabs, activeTab };
      });
      // Удаляем опустевшие группы, но всегда оставляем хотя бы одну.
      const nonEmpty = updated.filter((g) => g.tabs.length > 0);
      const groups = nonEmpty.length ? nonEmpty : freshGroups();
      const activeGroupId = groups.some((g) => g.id === s.activeGroupId)
        ? s.activeGroupId
        : groups[0].id;
      // GC: буферы без ссылок из вкладок.
      const referenced = new Set(groups.flatMap((g) => g.tabs));
      const buffers = Object.fromEntries(
        Object.entries(s.buffers).filter(([p]) => referenced.has(p)),
      );
      return { groups, activeGroupId, buffers };
    });
  },

  moveTab(fromGroupId, toGroupId, path) {
    if (fromGroupId === toGroupId) return;
    set((s) => {
      if (!s.groups.some((g) => g.id === toGroupId)) return {};
      const updated = s.groups.map((g) => {
        if (g.id === fromGroupId) {
          const tabs = g.tabs.filter((t) => t !== path);
          const activeTab = g.activeTab === path ? (tabs[tabs.length - 1] ?? null) : g.activeTab;
          return { ...g, tabs, activeTab };
        }
        if (g.id === toGroupId) {
          // Если вкладка уже есть в цели — не дублируем, просто активируем (контракт макета).
          const tabs = g.tabs.includes(path) ? g.tabs : [...g.tabs, path];
          return { ...g, tabs, activeTab: path };
        }
        return g;
      });
      const nonEmpty = updated.filter((g) => g.tabs.length > 0);
      const groups = nonEmpty.length ? nonEmpty : freshGroups();
      return { groups, activeGroupId: toGroupId };
    });
  },

  modes: {},
  toggleMode(groupId) {
    set((s) => {
      const gid = groupId ?? s.activeGroupId;
      const next = (s.modes[gid] ?? 'source') === 'source' ? 'preview' : 'source';
      return { modes: { ...s.modes, [gid]: next } };
    });
  },

  splitRight() {
    set((s) => {
      const active = s.groups.find((g) => g.id === s.activeGroupId);
      const tab = active?.activeTab ?? null;
      const id = nextGroupId();
      return {
        groups: [...s.groups, { id, tabs: tab ? [tab] : [], activeTab: tab }],
        activeGroupId: id,
      };
    });
  },

  updateBufferDoc(path, doc) {
    set((s) =>
      s.buffers[path]
        ? { buffers: { ...s.buffers, [path]: { ...s.buffers[path], doc, dirty: true } } }
        : {},
    );
  },

  async saveBuffer(path) {
    const buffer = get().buffers[path];
    if (!buffer) return;
    // Хеш записанного — новый baseHash: эхо собственного сейва не поднимет guard внешнего
    // изменения (SAFE-3). doc на момент записи фиксируем, чтобы baseHash соответствовал ему.
    const saved = buffer.doc;
    const hash = await tauriApi.vault.writeFile(path, saved);
    set((s) => {
      const b = s.buffers[path];
      if (!b) return {};
      // Если за время записи документ не менялся — снимаем dirty; иначе оставляем (есть новые правки).
      const stillSame = b.doc === saved;
      return {
        buffers: {
          ...s.buffers,
          [path]: { ...b, baseHash: hash, dirty: stillSame ? false : b.dirty },
        },
      };
    });
  },

  reset() {
    set({ buffers: {}, groups: freshGroups(), activeGroupId: INITIAL_GROUP, modes: {} });
  },
}));

/** Активный буфер (активная вкладка активной группы) — контекст AI/backlinks. */
export function activeBuffer(s: WorkspaceState): Buffer | null {
  const group = s.groups.find((g) => g.id === s.activeGroupId);
  if (!group?.activeTab) return null;
  return s.buffers[group.activeTab] ?? null;
}

/** Путь активной вкладки активной группы (для подсветки в дереве). */
export function activePath(s: WorkspaceState): string | null {
  const group = s.groups.find((g) => g.id === s.activeGroupId);
  return group?.activeTab ?? null;
}
