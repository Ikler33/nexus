import {
  autocompletion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
} from '@codemirror/autocomplete';
import { defaultHighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { markdown } from '@codemirror/lang-markdown';
import { type Extension, RangeSetBuilder } from '@codemirror/state';
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from '@codemirror/view';
import type { NoteRef } from '../../lib/tauri-api';
import { noteName } from '../../stores/vault';
import { slashSource } from './slashCommands';

// `[[Target]]`, `[[Target#H|Alias]]`, `![[Embed]]`
const WIKILINK_RE = /(!?)\[\[([^\]\n]+?)\]\]/g;
// `#tag` (минимум одна буква; перед — начало или пробел)
const TAG_RE = /(^|\s)(#[\p{L}\d/_-]*\p{L}[\p{L}\d/_-]*)/gu;

const wikilinkMark = Decoration.mark({ class: 'cm-wikilink' });
const tagMark = Decoration.mark({ class: 'cm-tag' });

/** Нормализует цель wiki-ссылки: срезает `|alias` и `#heading`. */
export function normalizeTarget(inner: string): string {
  return inner.split('|')[0].split('#')[0].trim();
}

interface DecoRange {
  from: number;
  to: number;
  deco: Decoration;
}

function collectDecorations(view: EditorView): DecorationSet {
  const ranges: DecoRange[] = [];
  for (const { from, to } of view.visibleRanges) {
    const text = view.state.sliceDoc(from, to);
    for (const m of text.matchAll(WIKILINK_RE)) {
      ranges.push({
        from: from + m.index!,
        to: from + m.index! + m[0].length,
        deco: wikilinkMark,
      });
    }
    for (const m of text.matchAll(TAG_RE)) {
      const start = m.index! + m[1].length;
      ranges.push({ from: from + start, to: from + start + m[2].length, deco: tagMark });
    }
  }
  ranges.sort((a, b) => a.from - b.from || a.to - b.to);
  const builder = new RangeSetBuilder<Decoration>();
  let last = -1;
  for (const r of ranges) {
    if (r.from >= last) {
      builder.add(r.from, r.to, r.deco);
      last = r.to;
    }
  }
  return builder.finish();
}

/** Подсветка `[[wikilink]]` и `#tag` (декорации поверх видимой области). */
const decorationPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = collectDecorations(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged) {
        this.decorations = collectDecorations(u.view);
      }
    }
  },
  { decorations: (v) => v.decorations },
);

/** Клик по `[[wikilink]]` → навигация (через `onOpenLink`). */
function wikilinkClick(onOpenLink: () => ((target: string) => void) | undefined): Extension {
  return EditorView.domEventHandlers({
    mousedown(event, view) {
      if (event.button !== 0) return false;
      const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
      if (pos == null) return false;
      const line = view.state.doc.lineAt(pos);
      const rel = pos - line.from;
      for (const m of line.text.matchAll(WIKILINK_RE)) {
        const start = m.index!;
        const end = start + m[0].length;
        if (rel >= start && rel <= end) {
          const target = normalizeTarget(m[2]);
          const handler = onOpenLink();
          if (target && handler) {
            event.preventDefault();
            handler(target);
            return true;
          }
        }
      }
      return false;
    },
  });
}

/** Автокомплит имён заметок внутри `[[…` — асинхронный запрос топ-N к бэкенду (кросс-план #22):
 * вместо полного списка vault в памяти каждый ввод спрашивает отфильтрованный срез (бэкенд ранжирует
 * префикс-совпадения выше). `filter: false` — CM6 не пере-фильтрует уже отобранное. Чистый
 * CompletionSource — монтируется в ЕДИНЫЙ autocompletion() рядом со slash-источником (EDIT-6). */
function wikilinkSource(fetchNotes: (q: string) => Promise<NoteRef[]>): CompletionSource {
  return async (ctx: CompletionContext): Promise<CompletionResult | null> => {
    const line = ctx.state.doc.lineAt(ctx.pos);
    const before = ctx.state.sliceDoc(line.from, ctx.pos);
    const m = /\[\[([^\]\n]*)$/.exec(before);
    if (!m) return null;
    const from = ctx.pos - m[1].length;
    const notes = await fetchNotes(m[1]);
    const options = notes.map((n) => ({
      label: noteName(n.path),
      detail: n.title ?? n.path,
      type: 'class',
    }));
    return { from, options, filter: false };
  };
}

const editorTheme = EditorView.theme({
  '&': { height: '100%', fontSize: 'var(--text-base)' },
  '.cm-scroller': { fontFamily: 'var(--font-editor)', lineHeight: 'var(--leading-normal)' },
  // Читаемая ширина строки (настройка «Редактор»): колонка ограничивается и центрируется через
  // CSS-переменную `--editor-max-width` (stores/prefs.ts). `none` → полная ширина (как было).
  '.cm-content': {
    padding: 'var(--space-4) 0',
    maxWidth: 'var(--editor-max-width, none)',
    marginInline: 'auto',
  },
  '.cm-wikilink': { color: 'var(--color-link)', cursor: 'pointer' },
  '.cm-tag': { color: 'var(--color-tag)' },
  // Inline-LLM ghost-text (IL-2): приглушённый курсивный текст предложения у курсора.
  '.cm-inline-ghost': {
    color: 'var(--color-text-faint)',
    fontStyle: 'italic',
    opacity: '0.75',
    whiteSpace: 'pre-wrap',
  },
  // Индикатор «генерируется» (до первого токена, AC-IL-1): акцентный пульсирующий чип.
  '.cm-inline-ghost-pending': {
    color: 'var(--color-accent)',
    opacity: '0.85',
    animation: 'nexus-ghost-pulse 1.1s ease-in-out infinite',
  },
  '@keyframes nexus-ghost-pulse': {
    '0%, 100%': { opacity: '0.4' },
    '50%': { opacity: '0.9' },
  },
  // Подсказка accept/reject у завершённого предложения (AC-IL-10).
  '.cm-inline-ghost-hint': {
    fontSize: 'var(--text-xs)',
    fontStyle: 'normal',
    opacity: '0.6',
    whiteSpace: 'nowrap',
  },
  // Inline-ошибка у курсора (AC-IL-7): без модала, ненавязчиво.
  '.cm-inline-ghost-error': {
    color: 'var(--color-danger, oklch(0.6 0.2 25))',
    fontStyle: 'normal',
    opacity: '0.95',
  },
  // Тулбар по выделению (IL-3, D4): чип с кнопками над выделением.
  '.cm-inline-toolbar': {
    display: 'flex',
    gap: '2px',
    padding: '3px',
    background: 'var(--color-bg-elevated)',
    border: '1px solid var(--color-border-strong)',
    borderRadius: 'var(--radius-md)',
    boxShadow: 'var(--elevation-2)',
  },
  '.cm-inline-toolbar-btn': {
    font: 'inherit',
    fontSize: 'var(--text-xs)',
    color: 'var(--color-text)',
    background: 'transparent',
    border: 'none',
    borderRadius: 'var(--radius-sm)',
    padding: '3px 8px',
    cursor: 'pointer',
  },
  '.cm-inline-toolbar-btn:hover': {
    background: 'var(--color-surface-hover)',
    color: 'var(--color-accent)',
  },
  '&.cm-focused': { outline: 'none' },
});

/** Колбэки редактора (через ref-геттеры — всегда актуальны без пересоздания view). */
export interface EditorCallbacks {
  /** Заметки по подстроке для автокомплита `[[…` (бэкенд-фильтр + лимит, #22). */
  fetchNotes: (query: string) => Promise<NoteRef[]>;
  getOpenLink: () => ((target: string) => void) | undefined;
}

/** Полный набор расширений source-mode редактора Nexus. */
export function nexusExtensions(cb: EditorCallbacks): Extension[] {
  return [
    markdown(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    decorationPlugin,
    wikilinkClick(cb.getOpenLink),
    // EDIT-6: ЕДИНЫЙ autocompletion() с двумя источниками — wikilink (`[[…`) и slash (`/…`). Их
    // контексты взаимоисключающие (по regex), монтировать два autocompletion() нельзя (конфликт конфигов).
    autocompletion({ override: [wikilinkSource(cb.fetchNotes), slashSource()] }),
    editorTheme,
    EditorView.lineWrapping,
  ];
}
