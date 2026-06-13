import { create } from 'zustand';
import { isViewable } from '../lib/file-kind';
import { tauriApi } from '../lib/tauri-api';
import { cancelAllAutosave, cancelAutosave, flush, scheduleAutosave } from './autosave';

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
  /** Файл изменился на диске, пока в буфере были несохранённые правки → баннер guard'а (SAFE-3).
   *  Чистый буфер перечитывается тихо (флаг не ставится). */
  externalChange?: boolean;
  /** Идёт запись на диск (SAFE-4) — индикатор «Сохранение…». */
  saving?: boolean;
  /** Метка времени последнего успешного сохранения (SAFE-4) — индикатор «Сохранено · …». */
  savedAt?: number;
  /** Текст ошибки последнего сохранения (SAFE-4): запись не удалась → dirty НЕ сброшен, правки целы,
   *  ошибка ВИДИМА (мандат 3). Тост/ретрай — в P4 (toast-система). */
  saveError?: string;
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
  /** Сохранить буфер. `manual` (Ctrl-S/палитра) — точка истории всегда; автосейв (false) — троттл. */
  saveBuffer: (path: string, manual?: boolean) => Promise<void>;
  /** Реакция на vault:file-changed (SAFE-3): эхо своего сейва — игнор; грязный буфер — баннер;
   *  чистый — тихий reload с диска. */
  onExternalFileChange: (path: string, hash: string) => Promise<void>;
  /** Перечитать буфер с диска (баннер «Загрузить с диска» или тихий reload чистого буфера). */
  reloadFromDisk: (path: string) => Promise<void>;
  /** «Оставить мои»: сдвинуть baseHash к текущему диску (следующий сейв перезапишет осознанно),
   *  снять баннер; правки и dirty сохраняются. */
  keepMine: (path: string) => Promise<void>;
  /** Выбросить буферы/вкладки удалённого пути (файл) или поддерева (каталог) — CURATE-1. */
  dropPathsUnder: (path: string) => void;
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
    // SAFE-4: флаш при уходе с вкладки покрывается blur редактора (клик по другой вкладке уводит
    // фокус из CM6 → onBlur→flush) + автосейвом по паузе. Здесь НЕ флашим — иначе сняли бы dirty при
    // каждом переключении (конфликт с AC-Б12-2: буфер сохраняет dirty между вкладками).
    set((s) => ({
      activeGroupId: groupId,
      groups: s.groups.map((g) => (g.id === groupId ? { ...g, activeTab: path } : g)),
    }));
  },

  setActiveGroup(groupId) {
    set({ activeGroupId: groupId });
  },

  closeTab(groupId, path) {
    // SAFE-4 (критфикс): closeTab GC-ил буфер БЕЗ записи — несохранённые правки терялись. Флашим
    // ПЕРЕД GC. flush читает буфер сейчас (ещё есть), saveBuffer фиксирует doc строкой → запись
    // переживёт удаление буфера.
    void flush(path);
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
    scheduleAutosave(path); // SAFE-4: сохранить через паузу набора (debounce 1с)
  },

  async saveBuffer(path, manual = false) {
    const buffer = get().buffers[path];
    if (!buffer || !buffer.dirty) return; // нечего сохранять (чисто/нет буфера)
    // Хеш записанного — новый baseHash: эхо собственного сейва не поднимет guard внешнего
    // изменения (SAFE-3). doc на момент записи фиксируем, чтобы baseHash соответствовал ему.
    const saved = buffer.doc;
    set((s) =>
      s.buffers[path]
        ? { buffers: { ...s.buffers, [path]: { ...s.buffers[path], saving: true, saveError: undefined } } }
        : {},
    );
    try {
      const hash = await tauriApi.vault.writeFile(path, saved, manual);
      set((s) => {
        const b = s.buffers[path];
        if (!b) return {};
        // Документ не менялся за время записи — снимаем dirty; иначе оставляем (есть новые правки).
        const stillSame = b.doc === saved;
        return {
          buffers: {
            ...s.buffers,
            [path]: {
              ...b,
              baseHash: hash,
              dirty: stillSame ? false : b.dirty,
              saving: false,
              savedAt: Date.now(),
              saveError: undefined,
            },
          },
        };
      });
    } catch (e) {
      // SAFE-4 (поправка критика): запись упала → dirty НЕ сбрасываем (правки целы), ошибку делаем
      // ВИДИМОЙ (мандат 3). Тост/ретрай — в P4 (toast-система); пока статусбар покажет «Ошибка».
      set((s) =>
        s.buffers[path]
          ? {
              buffers: {
                ...s.buffers,
                [path]: { ...s.buffers[path], saving: false, saveError: String(e) },
              },
            }
          : {},
      );
    }
  },

  async onExternalFileChange(path, hash) {
    const b = get().buffers[path];
    if (!b) return; // файл не открыт — игнор
    if (hash === b.baseHash) return; // эхо собственного сейва (тот же контент)
    if (b.dirty) {
      // Грязный буфер: содержимое НЕ трогаем (не теряем правки), показываем баннер.
      set((s) =>
        s.buffers[path]
          ? { buffers: { ...s.buffers, [path]: { ...s.buffers[path], externalChange: true } } }
          : {},
      );
    } else {
      // Чистый буфер: тихо перечитываем с диска (курсор сохранит Editor через externalSync).
      try {
        await get().reloadFromDisk(path);
      } catch {
        /* файл мог исчезнуть между событием и чтением — оставляем буфер как есть */
      }
    }
  },

  async reloadFromDisk(path) {
    const meta = await tauriApi.vault.readFileMeta(path);
    set((s) =>
      s.buffers[path]
        ? {
            buffers: {
              ...s.buffers,
              [path]: {
                ...s.buffers[path],
                doc: meta.content,
                baseHash: meta.hash,
                dirty: false,
                externalChange: false,
              },
            },
          }
        : {},
    );
  },

  async keepMine(path) {
    const hash = await tauriApi.vault.fileHash(path).catch(() => null);
    set((s) =>
      s.buffers[path]
        ? {
            buffers: {
              ...s.buffers,
              [path]: {
                ...s.buffers[path],
                baseHash: hash ?? s.buffers[path].baseHash,
                externalChange: false,
              },
            },
          }
        : {},
    );
  },

  dropPathsUnder(path) {
    const isUnder = (p: string) => p === path || p.startsWith(`${path}/`);
    for (const p of Object.keys(get().buffers)) {
      if (isUnder(p)) cancelAutosave(p); // не дать автосейву воскресить удалённый файл
    }
    set((s) => {
      const groups = s.groups.map((g) => {
        const tabs = g.tabs.filter((t) => !isUnder(t));
        const activeTab =
          g.activeTab && isUnder(g.activeTab) ? (tabs[tabs.length - 1] ?? null) : g.activeTab;
        return { ...g, tabs, activeTab };
      });
      const nonEmpty = groups.filter((g) => g.tabs.length > 0);
      const finalGroups = nonEmpty.length ? nonEmpty : freshGroups();
      const activeGroupId = finalGroups.some((g) => g.id === s.activeGroupId)
        ? s.activeGroupId
        : finalGroups[0].id;
      const referenced = new Set(finalGroups.flatMap((g) => g.tabs));
      const buffers = Object.fromEntries(
        Object.entries(s.buffers).filter(([p]) => referenced.has(p)),
      );
      return { groups: finalGroups, activeGroupId, buffers };
    });
  },

  reset() {
    cancelAllAutosave(); // SAFE-4: гасим отложенные автосейвы — не стреляют по выброшенным буферам
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
