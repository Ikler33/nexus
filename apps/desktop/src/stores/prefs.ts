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

/** MEM-8c-b: режим консолидации памяти. `propose` — каждое слияние/замещение через чип (безопасно по
 *  умолчанию); `auto` — применять автоматически (кроме explicit-фактов §4.3), с undo-тостом. */
export type ConsolidationMode = 'propose' | 'auto';
const CONSOLIDATION_MODE_KEY = 'nexus.ai.memoryConsolidationMode';

function readConsolidationMode(): ConsolidationMode {
  try {
    const v = localStorage.getItem(CONSOLIDATION_MODE_KEY);
    if (v === 'propose' || v === 'auto') return v;
  } catch {
    /* ignore */
  }
  return 'propose';
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

/** LLM-реранжирование RAG-источников (search::rerank, default ВКЛ — eval: nDCG .883→1.0). */
const AI_RERANK_KEY = 'nexus.ai.rerank';
const AI_CHAT_MEMORY_KEY = 'nexus.ai.chatMemory';
/** Память агента (MEM, явные факты) — ВЫКЛ по умолчанию (D5: приватность-first). */
const AI_AGENT_MEMORY_KEY = 'nexus.ai.agentMemory';
/** Эпизодическая память (EP, саммари сессий) — ВЫКЛ по умолчанию (приватность-first). */
const AI_EPISODIC_MEMORY_KEY = 'nexus.ai.episodicMemory';
/** Консолидация памяти (MEM-8) — ВЫКЛ по умолчанию (owner-gated: LLM сливает/замещает факты). */
const AI_MEMORY_CONSOLIDATION_KEY = 'nexus.ai.memoryConsolidation';
const AI_EXPLAIN_RELATIONS_KEY = 'nexus.ai.explainRelations';

function readBoolDefaultTrue(key: string): boolean {
  try {
    return localStorage.getItem(key) !== '0';
  } catch {
    return true;
  }
}

function readBoolDefaultFalse(key: string): boolean {
  try {
    return localStorage.getItem(key) === '1';
  } catch {
    return false;
  }
}

/** Размер AI-панели (фидбэк владельца 11.06: «увеличить окно с чатом») — перетаскивание кромки. */
const AI_W_KEY = 'nexus.ai.panelW';
const AI_H_KEY = 'nexus.ai.panelH';
export const AI_PANEL_W = { min: 300, def: 360, max: 720 };
export const AI_PANEL_H = { min: 200, def: 280, max: 560 };

function readNum(key: string, def: number, min: number, max: number): number {
  try {
    const v = Number(localStorage.getItem(key));
    if (Number.isFinite(v) && v >= min && v <= max) return v;
  } catch {
    /* ignore */
  }
  return def;
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
  /** LLM-реранжирование источников чата (точнее порядок, +~2 с на вопрос). */
  aiRerank: boolean;
  /** Память переписки (N4b): подмешивать релевантные фрагменты прошлых диалогов в контекст. ВКЛ. */
  aiChatMemory: boolean;
  /** Память агента (MEM): подмешивать сохранённые ЯВНЫЕ ФАКТЫ о пользователе/проектах (пины + top-k)
   *  в контекст ответа. ВЫКЛ по умолчанию (D5: приватность-first); тумблер в Настройках → AI. */
  aiAgentMemory: boolean;
  /** Эпизодическая память (EP): подмешивать саммари релевантных прошлых СЕССИЙ в контекст ответа.
   *  ВЫКЛ по умолчанию (приватность-first); тумблер в Настройках → AI — EP-3. */
  aiEpisodicMemory: boolean;
  /** Консолидация памяти (MEM-8): при подтверждении факта ИИ предлагает объединить/заменить близкий
   *  существующий (режим «Предлагать» — каждое слияние/замещение через подтверждение, обратимо). ВЫКЛ
   *  по умолчанию (owner-gated). Работает только при `aiAgentMemory` + наличии основной модели. */
  aiMemoryConsolidation: boolean;
  /** MEM-8c-b: режим консолидации (`propose`|`auto`). Имеет смысл при `aiMemoryConsolidation` ON. */
  aiMemoryConsolidationMode: ConsolidationMode;
  /** AIP-10: LLM-объяснение «причины связи» в «Связях»/«Похожих» вместо сырого сниппета (лениво,
   *  кэш). ВКЛ; реально работает при наличии утилитарной модели (иначе фолбэк на сниппет). */
  aiExplainRelations: boolean;
  /** Ширина side-AI-панели, px (драг кромки). */
  aiPanelW: number;
  /** Высота bottom-AI-панели, px (драг кромки). */
  aiPanelH: number;
  setReadableLineWidth: (on: boolean) => void;
  setUserName: (name: string) => void;
  setPaletteStyle: (style: PaletteStyle) => void;
  setAiLayout: (layout: AiLayout) => void;
  setRagSources: (style: RagSources) => void;
  setAiRerank: (on: boolean) => void;
  setAiChatMemory: (on: boolean) => void;
  setAiAgentMemory: (on: boolean) => void;
  setAiEpisodicMemory: (on: boolean) => void;
  setAiMemoryConsolidation: (on: boolean) => void;
  setAiMemoryConsolidationMode: (mode: ConsolidationMode) => void;
  setAiExplainRelations: (on: boolean) => void;
  setAiPanelW: (w: number) => void;
  setAiPanelH: (h: number) => void;
}

export const usePrefsStore = create<PrefsState>((set) => ({
  readableLineWidth: START_READABLE,
  userName: readString(USER_NAME_KEY),
  paletteStyle: readPalette(),
  aiLayout: readAiLayout(),
  ragSources: readRagSources(),
  aiRerank: readBoolDefaultTrue(AI_RERANK_KEY),
  aiChatMemory: readBoolDefaultTrue(AI_CHAT_MEMORY_KEY),
  aiAgentMemory: readBoolDefaultFalse(AI_AGENT_MEMORY_KEY),
  aiEpisodicMemory: readBoolDefaultFalse(AI_EPISODIC_MEMORY_KEY),
  aiMemoryConsolidation: readBoolDefaultFalse(AI_MEMORY_CONSOLIDATION_KEY),
  aiMemoryConsolidationMode: readConsolidationMode(),
  aiExplainRelations: readBoolDefaultTrue(AI_EXPLAIN_RELATIONS_KEY),
  aiPanelW: readNum(AI_W_KEY, AI_PANEL_W.def, AI_PANEL_W.min, AI_PANEL_W.max),
  aiPanelH: readNum(AI_H_KEY, AI_PANEL_H.def, AI_PANEL_H.min, AI_PANEL_H.max),
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
  setAiRerank: (on) => {
    try {
      localStorage.setItem(AI_RERANK_KEY, on ? '1' : '0');
    } catch {
      /* ignore */
    }
    set({ aiRerank: on });
  },
  setAiChatMemory: (on) => {
    try {
      localStorage.setItem(AI_CHAT_MEMORY_KEY, on ? '1' : '0');
    } catch {
      /* ignore */
    }
    set({ aiChatMemory: on });
  },
  setAiAgentMemory: (on) => {
    try {
      localStorage.setItem(AI_AGENT_MEMORY_KEY, on ? '1' : '0');
    } catch {
      /* ignore */
    }
    set({ aiAgentMemory: on });
  },
  setAiEpisodicMemory: (on) => {
    try {
      localStorage.setItem(AI_EPISODIC_MEMORY_KEY, on ? '1' : '0');
    } catch {
      /* ignore */
    }
    set({ aiEpisodicMemory: on });
  },
  setAiMemoryConsolidation: (on) => {
    try {
      localStorage.setItem(AI_MEMORY_CONSOLIDATION_KEY, on ? '1' : '0');
    } catch {
      /* ignore */
    }
    set({ aiMemoryConsolidation: on });
  },
  setAiMemoryConsolidationMode: (mode) => {
    try {
      localStorage.setItem(CONSOLIDATION_MODE_KEY, mode);
    } catch {
      /* ignore */
    }
    set({ aiMemoryConsolidationMode: mode });
  },
  setAiExplainRelations: (on) => {
    try {
      localStorage.setItem(AI_EXPLAIN_RELATIONS_KEY, on ? '1' : '0');
    } catch {
      /* ignore */
    }
    set({ aiExplainRelations: on });
  },
  setAiPanelW: (w) => {
    const v = Math.round(Math.min(AI_PANEL_W.max, Math.max(AI_PANEL_W.min, w)));
    try {
      localStorage.setItem(AI_W_KEY, String(v));
    } catch {
      /* ignore */
    }
    set({ aiPanelW: v });
  },
  setAiPanelH: (h) => {
    const v = Math.round(Math.min(AI_PANEL_H.max, Math.max(AI_PANEL_H.min, h)));
    try {
      localStorage.setItem(AI_H_KEY, String(v));
    } catch {
      /* ignore */
    }
    set({ aiPanelH: v });
  },
}));
