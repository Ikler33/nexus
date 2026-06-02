import { useEffect, useRef } from 'react';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { Annotation, EditorState } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import type { NoteRef } from '../../lib/tauri-api';
import { nexusExtensions } from './extensions';
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

    const view = new EditorView({
      state: EditorState.create({
        doc: loadedDoc.current,
        extensions: [
          history(),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
          saveKey,
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

  // Смена файла → заменяем документ одним dispatch (view НЕ пересоздаётся).
  useEffect(() => {
    const view = viewRef.current;
    if (!view || loadedPath.current === path) return;
    loadedPath.current = path;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: initialDoc },
      selection: { anchor: 0 },
      annotations: externalSync.of(true),
    });
  }, [path, initialDoc]);

  return <div ref={host} className={styles.editor} data-testid="editor" />;
}
