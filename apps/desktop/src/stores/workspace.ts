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

/** Запись истории навигации (NAV-3): путь + группа, в которой он был открыт (для возврата в
 *  родную панель в мульти-пейне; группа могла схлопнуться → фолбэк на активную). */
export interface NavEntry {
  path: string;
  groupId: string;
}

const INITIAL_GROUP = 'g0';
let groupSeq = 0;
const nextGroupId = () => `g${++groupSeq}`;

/** Список недавно открытых заметок (NAV-2, ⌘O quick-switcher): MRU, без дублей, кап 20.
 *  Персистится в localStorage — быстрый возврат переживает перезапуск. Глобальный ключ
 *  (приложение однохранилищное); reset (закрытие vault) чистит in-memory. */
const RECENTS_KEY = 'nexus.recents.v1';
const RECENTS_MAX = 20;

/** Глубина истории навигации (NAV-3, back/forward ⌘[ / ⌘]). */
const NAV_MAX = 50;

function loadRecents(): string[] {
  try {
    if (typeof localStorage === 'undefined') return [];
    const raw = localStorage.getItem(RECENTS_KEY);
    const arr = raw ? JSON.parse(raw) : [];
    return Array.isArray(arr)
      ? arr.filter((p): p is string => typeof p === 'string').slice(0, RECENTS_MAX)
      : [];
  } catch {
    return [];
  }
}

function saveRecents(list: string[]): void {
  try {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(RECENTS_KEY, JSON.stringify(list));
  } catch {
    /* приватный режим / квота — recents не критичны для работы */
  }
}

interface WorkspaceState {
  buffers: Record<string, Buffer>;
  groups: EditorGroup[];
  activeGroupId: string;
  /** Недавно открытые заметки (NAV-2): MRU-список путей для ⌘O quick-switcher. */
  recents: string[];
  /** История навигации (NAV-3): посещённые документы для back/forward (браузерная модель). */
  navHistory: NavEntry[];
  /** Курсор в navHistory (-1 = пусто). back уменьшает, forward увеличивает. */
  navIndex: number;

  /** `fromNav` — переход инициирован самим back/forward: не записываем его в историю заново. */
  openFile: (path: string, groupId?: string, opts?: { fromNav?: boolean }) => Promise<void>;
  /** Поднять путь в начало recents (дедуп, кап 20) + персист. Зовётся из openFile. */
  pushRecent: (path: string) => void;
  /** Внутренняя (NAV-3): записать переход (путь+группа) в историю, обрезав «вперёд»-хвост. */
  recordNav: (path: string, groupId: string) => void;
  /** Назад по истории навигации (⌘[). No-op на левом крае. */
  navBack: () => Promise<void>;
  /** Вперёд по истории навигации (⌘]). No-op на правом крае. */
  navForward: () => Promise<void>;
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
  /** Перенести открытые буферы/вкладки при rename/move пути (своп префикса) — CURATE-2. */
  renameBufferPath: (from: string, to: string) => void;
  reset: () => void;
}

function freshGroups(): EditorGroup[] {
  return [{ id: INITIAL_GROUP, tabs: [], activeTab: null }];
}

/** NAV-3: целевая группа для возврата записи истории — её родная группа, либо активная,
 *  если та схлопнулась (moveTab удаляет опустевшие группы). */
function navGroup(get: () => WorkspaceState, groupId: string): string {
  const s = get();
  return s.groups.some((g) => g.id === groupId) ? groupId : s.activeGroupId;
}

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  buffers: {},
  groups: freshGroups(),
  activeGroupId: INITIAL_GROUP,
  recents: loadRecents(),
  navHistory: [],
  navIndex: -1,

  pushRecent(path) {
    const recents = [path, ...get().recents.filter((p) => p !== path)].slice(0, RECENTS_MAX);
    saveRecents(recents);
    set({ recents });
  },

  recordNav(path, groupId) {
    const { navHistory, navIndex } = get();
    const cur = navHistory[navIndex];
    if (cur && cur.path === path && cur.groupId === groupId) return; // та же запись — не плодим
    const trimmed = navHistory.slice(0, navIndex + 1); // обрезаем «вперёд»-хвост (браузерная модель)
    trimmed.push({ path, groupId });
    const next = trimmed.slice(Math.max(0, trimmed.length - NAV_MAX)); // кап глубины
    set({ navHistory: next, navIndex: next.length - 1 });
  },

  async navBack() {
    const { navIndex, navHistory } = get();
    if (navIndex <= 0) return;
    // navIndex сдвигаем ТОЛЬКО после успешного openFile: если целевой файл удалён/переименован,
    // openFile реджектится — курсор остаётся консистентным с реально активным документом (а не
    // уезжает на мёртвую запись). Группу берём из записи (фолбэк на активную, если схлопнулась).
    const e = navHistory[navIndex - 1];
    try {
      await get().openFile(e.path, navGroup(get, e.groupId), { fromNav: true });
      set({ navIndex: navIndex - 1 });
    } catch {
      /* целевой файл недоступен — курсор не двигаем (запись подчистится при delete/rename) */
    }
  },

  async navForward() {
    const { navIndex, navHistory } = get();
    if (navIndex >= navHistory.length - 1) return;
    const e = navHistory[navIndex + 1];
    try {
      await get().openFile(e.path, navGroup(get, e.groupId), { fromNav: true });
      set({ navIndex: navIndex + 1 });
    } catch {
      /* целевой файл недоступен — курсор не двигаем */
    }
  },

  async openFile(path, groupId, opts) {
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
    get().pushRecent(path); // NAV-2: открытие = недавнее (для ⌘O)
    if (!opts?.fromNav) get().recordNav(path, gid); // NAV-3: запись в историю (кроме back/forward)
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
    get().recordNav(path, groupId); // NAV-3: переключение вкладки — тоже навигация (для back/forward)
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
      // NAV-3: выбрасываем удалённые пути из истории (иначе navBack упёрся бы в мёртвый путь → reject).
      const navHistory = s.navHistory.filter((e) => !isUnder(e.path));
      // Курсор держим на записи РЕАЛЬНО активного документа (инвариант navHistory[navIndex].path ===
      // activePath): dropPathsUnder выбирает новый activeTab как правый-выживший, а простой кламп влево
      // мог бы указать на другую запись. Берём вхождение активного пути, ближайшее к прежней позиции.
      const removedUpTo = s.navHistory.slice(0, s.navIndex + 1).filter((e) => isUnder(e.path)).length;
      const clamped = Math.max(-1, Math.min(s.navIndex - removedUpTo, navHistory.length - 1));
      const activeNow = finalGroups.find((g) => g.id === activeGroupId)?.activeTab ?? null;
      let navIndex = clamped;
      if (activeNow !== null && navHistory[clamped]?.path !== activeNow) {
        let left = -1;
        let right = -1;
        for (let i = 0; i < navHistory.length; i++) {
          if (navHistory[i].path !== activeNow) continue;
          if (i <= clamped) left = i; // последняя ≤ clamped
          else if (right === -1) right = i; // первая > clamped
        }
        if (left !== -1) navIndex = left;
        else if (right !== -1) navIndex = right;
      }
      const recents = s.recents.filter((p) => !isUnder(p)); // NAV-2: и из недавних тоже (нет мёртвых)
      return { groups: finalGroups, activeGroupId, buffers, navHistory, navIndex, recents };
    });
    saveRecents(get().recents); // персист подчищенных recents
  },

  renameBufferPath(from, to) {
    // Своп пути для самого файла и для всего поддерева (каталог): from/x.md → to/x.md.
    const map = (p: string): string =>
      p === from ? to : p.startsWith(`${from}/`) ? `${to}${p.slice(from.length)}` : p;
    set((s) => {
      const buffers: Record<string, Buffer> = {};
      for (const [p, b] of Object.entries(s.buffers)) {
        const np = map(p);
        buffers[np] = np === p ? b : { ...b, path: np };
      }
      const groups = s.groups.map((g) => ({
        ...g,
        tabs: g.tabs.map(map),
        activeTab: g.activeTab ? map(g.activeTab) : null,
      }));
      // NAV-3: ремапим пути в истории навигации (длина/порядок сохраняются → navIndex не трогаем),
      // иначе back/forward на переименованную заметку ушёл бы на старый путь → reject openFile.
      const navHistory = s.navHistory.map((e) => ({ ...e, path: map(e.path) }));
      // NAV-2: те же пути в недавних; дедуп на случай rename на уже-недавний путь (коллизия имён).
      const seen = new Set<string>();
      const recents: string[] = [];
      for (const p of s.recents.map(map)) {
        if (!seen.has(p)) {
          seen.add(p);
          recents.push(p);
        }
      }
      return { buffers, groups, navHistory, recents };
    });
    saveRecents(get().recents); // персист ремапленных recents
  },

  reset() {
    cancelAllAutosave(); // SAFE-4: гасим отложенные автосейвы — не стреляют по выброшенным буферам
    set({
      buffers: {},
      groups: freshGroups(),
      activeGroupId: INITIAL_GROUP,
      modes: {},
      recents: [],
      navHistory: [],
      navIndex: -1,
    });
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
