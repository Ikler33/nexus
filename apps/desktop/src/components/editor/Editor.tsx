import { useEffect, useRef } from 'react';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { Annotation, EditorState } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import type { NoteRef } from '../../lib/tauri-api';
import { useInlineStore } from '../../stores/inline';
import { nexusExtensions } from './extensions';
import { ghostField, inlineKeymap } from './inlineGhost';
import styles from './Editor.module.css';

/** Помечает программную замену документа (смена файла) — НЕ пользовательскую правку. */
const externalSync = Annotation.define<boolean>();

export interface EditorProps {
  /** Идентичность буфера: смена `path` → перезагрузка документа через dispatch. */
  path: string;
  initialDoc: string;
  onChange?: (doc: string) => void;
  onSave?: (doc: string) => void;
  onOpenLink?: (target: string) => void;
  getNotes?: () => NoteRef[];
}

/**
 * Source-mode редактор на CodeMirror 6. Контракт CM6↔React (§4.1/Ф0): `EditorView`
 * создаётся ОДИН раз; смена файла — через `dispatch` (без пересоздания); StrictMode-
 * двойной mount гасится cleanup'ом + guard'ом. Колбэки берутся из ref → актуальны без
 * перестройки расширений.
 */
export function Editor({
  path,
  initialDoc,
  onChange,
  onSave,
  onOpenLink,
  getNotes,
}: EditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const cb = useRef({ onChange, onSave, onOpenLink, getNotes });
  cb.current = { onChange, onSave, onOpenLink, getNotes };
  const loadedPath = useRef(path);
  const loadedDoc = useRef(initialDoc);
  loadedDoc.current = initialDoc;

  useEffect(() => {
    const parent = host.current;
    if (!parent || viewRef.current) return; // guard StrictMode

    const saveKey = keymap.of([
      {
        key: 'Mod-s',
        preventDefault: true,
        run: (view) => {
          cb.current.onSave?.(view.state.doc.toString());
          return true;
        },
      },
    ]);

    // Inline-LLM (IL-2): триггер «продолжить» у курсора (Mod-i). Slash-меню / тулбар по выделению (D4/D5)
    // — IL-3. Tab/Esc (accept/reject) ставит `inlineKeymap` (Prec.highest, перехват только при ghost).
    const inlineTrigger = keymap.of([
      {
        key: 'Mod-i',
        preventDefault: true,
        run: (view) => {
          useInlineStore.getState().runInline(view, 'continue');
          return true;
        },
      },
    ]);

    const view = new EditorView({
      state: EditorState.create({
        doc: loadedDoc.current,
        extensions: [
          history(),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
          saveKey,
          inlineTrigger,
          ghostField,
          inlineKeymap({ onResolve: () => useInlineStore.getState().cancelInline() }),
          ...nexusExtensions({
            getNotes: () => cb.current.getNotes?.() ?? [],
            getOpenLink: () => cb.current.onOpenLink,
          }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged && !u.transactions.some((t) => t.annotation(externalSync))) {
              cb.current.onChange?.(u.state.doc.toString());
            }
          }),
        ],
      }),
      parent,
    });
    viewRef.current = view;
    loadedPath.current = path;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Замена документа одним dispatch (view НЕ пересоздаётся): при смене файла ИЛИ внешнем изменении
  // того же файла (accept-связь Ф1-9, watcher-reload). Эхо собственной правки (initialDoc уже равен
  // содержимому view) игнорируем — иначе цикл и прыжок курсора при наборе. `externalSync` → не dirty.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const switching = loadedPath.current !== path;
    if (!switching && initialDoc === view.state.doc.toString()) return;
    loadedPath.current = path;
    const anchor = switching
      ? 0
      : Math.min(view.state.selection.main.anchor, initialDoc.length);
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: initialDoc },
      selection: { anchor },
      annotations: externalSync.of(true),
    });
  }, [path, initialDoc]);

  return <div ref={host} className={styles.editor} data-testid="editor" />;
}
