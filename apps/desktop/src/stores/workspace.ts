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
      // Бинарь (картинка/PDF) не читаем как текст — его покажет FileViewer (asset-URL).
      const doc = isViewable(path) ? '' : await tauriApi.vault.readFile(path);
      buffers = { ...buffers, [path]: { path, doc, dirty: false } };
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
    await tauriApi.vault.writeFile(path, buffer.doc);
    set((s) =>
      s.buffers[path]
        ? { buffers: { ...s.buffers, [path]: { ...s.buffers[path], dirty: false } } }
        : {},
    );
  },

  reset() {
    set({ buffers: {}, groups: freshGroups(), activeGroupId: INITIAL_GROUP });
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
