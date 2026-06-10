import { create } from 'zustand';

/**
 * Пользовательские настройки, не относящиеся к оформлению (theme-стор) или языку (i18n): сейчас —
 * «читаемая ширина строки» редактора (как Obsidian «Readable line length»). Применяется CSS-переменной
 * `--editor-max-width` на `<html>` (её читает тема редактора `.cm-content`), персист в localStorage,
 * стартовое применение на импорте модуля (без вспышки) — тот же приём, что в theme-сторе (`main.tsx`).
 */
const READABLE_KEY = 'nexus.editor.readableWidth';
/** Ширина читаемой колонки (~700px, как дефолт Obsidian). */
const READABLE_WIDTH = '44rem';

function readBool(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key);
    if (v === 'true') return true;
    if (v === 'false') return false;
  } catch {
    /* localStorage недоступен */
  }
  return fallback;
}

function persistBool(key: string, value: boolean): void {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    /* ignore */
  }
}

function applyReadable(on: boolean): void {
  if (typeof document !== 'undefined') {
    document.documentElement.style.setProperty('--editor-max-width', on ? READABLE_WIDTH : 'none');
  }
}

// По умолчанию ВКЛ (как Obsidian) — комфортнее читать; пользователь может выключить.
const START_READABLE = readBool(READABLE_KEY, true);
applyReadable(START_READABLE);

/** Имя для приветствия HOME (DP-1, «Добрый день, …»); пусто — приветствие без имени. */
const USER_NAME_KEY = 'nexus.user.name';

/** Позиция палитры (DP-11, макет tweaks): top / center / spotlight. */
export type PaletteStyle = 'top' | 'center' | 'spotlight';
const PALETTE_KEY = 'nexus.palette.style';

function readPalette(): PaletteStyle {
  try {
    const v = localStorage.getItem(PALETTE_KEY);
    if (v === 'top' || v === 'center' || v === 'spotlight') return v;
  } catch {
    /* ignore */
  }
  return 'top';
}

/** Расположение AI-панели (DP-12, макет tweaks): side / bottom / overlay. */
export type AiLayout = 'side' | 'bottom' | 'overlay';
const AI_LAYOUT_KEY = 'nexus.ai.layout';

function readAiLayout(): AiLayout {
  try {
    const v = localStorage.getItem(AI_LAYOUT_KEY);
    if (v === 'side' || v === 'bottom' || v === 'overlay') return v;
  } catch {
    /* ignore */
  }
  return 'side';
}

/** Стиль RAG-источников в чате (DP-12, макет tweaks): cards / chips / footnotes. */
export type RagSources = 'cards' | 'chips' | 'footnotes';
const RAG_SOURCES_KEY = 'nexus.ai.ragSources';

function readRagSources(): RagSources {
  try {
    const v = localStorage.getItem(RAG_SOURCES_KEY);
    if (v === 'cards' || v === 'chips' || v === 'footnotes') return v;
  } catch {
    /* ignore */
  }
  return 'cards';
}

function readString(key: string): string {
  try {
    return localStorage.getItem(key) ?? '';
  } catch {
    return '';
  }
}

interface PrefsState {
  readableLineWidth: boolean;
  /** Имя пользователя для приветствия HOME (необязательное, локальное). */
  userName: string;
  /** Позиция командной палитры (DP-11). */
  paletteStyle: PaletteStyle;
  /** Расположение AI-панели (DP-12). */
  aiLayout: AiLayout;
  /** Стиль RAG-источников в чате (DP-12). */
  ragSources: RagSources;
  setReadableLineWidth: (on: boolean) => void;
  setUserName: (name: string) => void;
  setPaletteStyle: (style: PaletteStyle) => void;
  setAiLayout: (layout: AiLayout) => void;
  setRagSources: (style: RagSources) => void;
}

export const usePrefsStore = create<PrefsState>((set) => ({
  readableLineWidth: START_READABLE,
  userName: readString(USER_NAME_KEY),
  paletteStyle: readPalette(),
  aiLayout: readAiLayout(),
  ragSources: readRagSources(),
  setReadableLineWidth: (on) =>
    set(() => {
      persistBool(READABLE_KEY, on);
      applyReadable(on);
      return { readableLineWidth: on };
    }),
  setUserName: (name) =>
    set(() => {
      try {
        localStorage.setItem(USER_NAME_KEY, name);
      } catch {
        /* ignore */
      }
      return { userName: name };
    }),
  setPaletteStyle: (style) =>
    set(() => {
      try {
        localStorage.setItem(PALETTE_KEY, style);
      } catch {
        /* ignore */
      }
      return { paletteStyle: style };
    }),
  setAiLayout: (layout) =>
    set(() => {
      try {
        localStorage.setItem(AI_LAYOUT_KEY, layout);
      } catch {
        /* ignore */
      }
      return { aiLayout: layout };
    }),
  setRagSources: (style) =>
    set(() => {
      try {
        localStorage.setItem(RAG_SOURCES_KEY, style);
      } catch {
        /* ignore */
      }
      return { ragSources: style };
    }),
}));
