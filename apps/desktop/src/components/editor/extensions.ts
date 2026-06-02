import {
  autocompletion,
  type CompletionContext,
  type CompletionResult,
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

/** Автокомплит имён заметок внутри `[[…`. */
function wikilinkAutocomplete(getNotes: () => NoteRef[]): Extension {
  return autocompletion({
    override: [
      (ctx: CompletionContext): CompletionResult | null => {
        const line = ctx.state.doc.lineAt(ctx.pos);
        const before = ctx.state.sliceDoc(line.from, ctx.pos);
        const m = /\[\[([^\]\n]*)$/.exec(before);
        if (!m) return null;
        const from = ctx.pos - m[1].length;
        const options = getNotes().map((n) => ({
          label: noteName(n.path),
          detail: n.title ?? n.path,
          type: 'class',
        }));
        return { from, options, validFor: /[^\]\n]*/ };
      },
    ],
  });
}

const editorTheme = EditorView.theme({
  '&': { height: '100%', fontSize: 'var(--text-base)' },
  '.cm-scroller': { fontFamily: 'var(--font-editor)', lineHeight: 'var(--leading-normal)' },
  '.cm-content': { padding: 'var(--space-4) 0' },
  '.cm-wikilink': { color: 'var(--color-link)', cursor: 'pointer' },
  '.cm-tag': { color: 'var(--color-tag)' },
  '&.cm-focused': { outline: 'none' },
});

/** Колбэки редактора (через ref-геттеры — всегда актуальны без пересоздания view). */
export interface EditorCallbacks {
  getNotes: () => NoteRef[];
  getOpenLink: () => ((target: string) => void) | undefined;
}

/** Полный набор расширений source-mode редактора Nexus. */
export function nexusExtensions(cb: EditorCallbacks): Extension[] {
  return [
    markdown(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    decorationPlugin,
    wikilinkClick(cb.getOpenLink),
    wikilinkAutocomplete(cb.getNotes),
    editorTheme,
    EditorView.lineWrapping,
  ];
}
