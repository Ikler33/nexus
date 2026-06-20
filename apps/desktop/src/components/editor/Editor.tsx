import { useEffect, useRef } from 'react';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { markdownKeymap } from '@codemirror/lang-markdown';
import { highlightSelectionMatches, search, searchKeymap } from '@codemirror/search';
import { Annotation, EditorState, Prec } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import { clearActiveEditorView, setActiveEditorView } from '../../lib/editor/activeView';
import type { NoteRef } from '../../lib/tauri-api';
import { useInlineStore } from '../../stores/inline';
import { useInlineAIStore } from '../../stores/inlineAI';
import { useWorkspaceStore } from '../../stores/workspace';
import { nexusExtensions } from './extensions';
import { imagePaste } from '../../lib/editor/imagePaste';
import { ghostField, inlineKeymap } from './inlineGhost';
import { inlineToolbar } from './inlineToolbar';
import styles from './Editor.module.css';

/** Помечает программную замену документа (смена файла) — НЕ пользовательскую правку. */
const externalSync = Annotation.define<boolean>();

export interface EditorProps {
  /** Идентичность буфера: смена `path` → перезагрузка документа через dispatch. */
  path: string;
  /** Группа-владелец (для InlineAI ⌘/: открыть prompt-box именно в этой группе). */
  groupId: string;
  initialDoc: string;
  onChange?: (doc: string) => void;
  onSave?: (doc: string) => void;
  /** Потеря фокуса редактором (SAFE-4): немедленный flush несохранённых правок. */
  onBlur?: () => void;
  onOpenLink?: (target: string) => void;
  /** Заметки по подстроке для автокомплита `[[…` (бэкенд-фильтр + лимит, #22). */
  fetchNotes?: (query: string) => Promise<NoteRef[]>;
  /** Имена тегов vault для автокомплита `#tag` / `tags:` (PROP-4). */
  fetchTags?: () => Promise<string[]>;
}

/**
 * Source-mode редактор на CodeMirror 6. Контракт CM6↔React (§4.1/Ф0): `EditorView`
 * создаётся ОДИН раз; смена файла — через `dispatch` (без пересоздания); StrictMode-
 * двойной mount гасится cleanup'ом + guard'ом. Колбэки берутся из ref → актуальны без
 * перестройки расширений.
 */
export function Editor({
  path,
  groupId,
  initialDoc,
  onChange,
  onSave,
  onBlur,
  onOpenLink,
  fetchNotes,
  fetchTags,
}: EditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const cb = useRef({ onChange, onSave, onBlur, onOpenLink, fetchNotes, fetchTags });
  cb.current = { onChange, onSave, onBlur, onOpenLink, fetchNotes, fetchTags };
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

    // Inline-LLM: триггер «продолжить» у курсора (Mod-i, IL-2). Тулбар по выделению (D4) — `inlineToolbar`
    // (IL-3); команды палитры — через реестр активного view. Tab/Esc (accept/reject) — `inlineKeymap`
    // (Prec.highest, перехват только при активном ghost, AC-IL-5).
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

    // InlineAI prompt-box (⌘/, дизайн Qasr): свободный AI-запрос → вставка. Перехватываем В РЕДАКТОРЕ
    // (preventDefault → глобальный useKeymap НЕ откроет шпаргалку: она остаётся на ⌘/ вне редактора +
    // в палитре). Mod-/ ВЫРЕЗАН из defaultKeymap (toggleComment) ниже — конфликта внутри CM нет.
    // groupId константен на жизнь инстанса (Editor с `key={groupId}`), безопасно захватить в замыкании.
    const inlineAiTrigger = keymap.of([
      {
        key: 'Mod-/',
        preventDefault: true,
        run: (view) => {
          // Целимся вставкой в ЭТОТ view (а не глобально активный — важно при сплитах).
          useInlineAIStore.getState().open(groupId, view);
          return true;
        },
      },
    ]);

    // Регистрируем активный редактор для команд палитры (IL-3): фокус → этот view становится целью.
    const focusTracker = EditorView.domEventHandlers({
      focus: (_e, view) => {
        setActiveEditorView(view);
        return false;
      },
      blur: (_e, view) => {
        // SAFE-4: фокус ушёл из редактора → флаш. Проверяем на след. тике, что фокус реально потерян
        // (не внутренняя перестановка внутри CM6 — тогда view.hasFocus снова true).
        setTimeout(() => {
          if (!view.hasFocus) cb.current.onBlur?.();
        }, 0);
        return false;
      },
    });

    // NAV-3: уступаем ⌘[ / ⌘] навигации back/forward (глобальный useKeymap). Из defaultKeymap CM6
    // это indentLess/indentMore — иначе ⌘[ в фокусе редактора и сдвигал бы отступ, и навигировал
    // (порча текста). Отступ остаётся на Tab/Shift-Tab (indentWithTab).
    // POLISH: ⌘/ уступаем шпаргатке хоткеев (toggleComment CM6 иначе сработал бы И открыл бы её).
    const baseKeymap = defaultKeymap.filter(
      (b) => b.key !== 'Mod-[' && b.key !== 'Mod-]' && b.key !== 'Mod-/',
    );

    // EDIT-3: умное продолжение списков/тасков/цитат. Штатные команды @codemirror/lang-markdown:
    // Enter → insertNewlineContinueMarkup (продолжает `- `/`* `/`1. `/`> `, чекбокс `- [ ] ` свежим,
    // нумерацию инкрементом; на пустом пункте — выходит из списка, убирая маркер), Backspace →
    // deleteMarkupBackward (в начале пункта стирает маркер). Prec.high — перехват ДО defaultKeymap;
    // на не-списочной строке команды возвращают false → обычная вставка строки / удаление символа.
    const markupKeymap = Prec.high(keymap.of(markdownKeymap));

    // NAV-4: при создании view (новая панель/сплит, реоткрытие открытого буфера) восстанавливаем
    // сохранённую позицию курсора — иначе сплит той же заметки открылся бы в начале.
    const savedCursor = useWorkspaceStore.getState().buffers[path]?.cursor;
    const initSelection =
      savedCursor != null
        ? { anchor: Math.min(savedCursor, loadedDoc.current.length) }
        : undefined;

    const view = new EditorView({
      state: EditorState.create({
        doc: loadedDoc.current,
        selection: initSelection,
        extensions: [
          history(),
          markupKeymap,
          keymap.of([...baseKeymap, ...historyKeymap, indentWithTab]),
          // Поиск/замена в заметке (⌘F / панель с заменой) — стандартная панель CM6 (DOM-средствами,
          // CSP-безопасно, без сети/бэкенда). Убираем бинды, которым нужен мультикурсор (его НЕ включаем,
          // отложен на отдельный визуальный срез): `Mod-d` selectNextOccurrence и `Mod-Shift-l`
          // selectSelectionMatches (без allowMultipleSelections тихо схлопываются в одно выделение).
          search({ top: true }),
          highlightSelectionMatches(),
          keymap.of(searchKeymap.filter((b) => b.key !== 'Mod-d' && b.key !== 'Mod-Shift-l')),
          saveKey,
          inlineTrigger,
          inlineAiTrigger,
          ghostField,
          inlineKeymap({ onResolve: () => useInlineStore.getState().cancelInline() }),
          inlineToolbar,
          focusTracker,
          imagePaste(),
          ...nexusExtensions({
            fetchNotes: (q) => cb.current.fetchNotes?.(q) ?? Promise.resolve([]),
            fetchTags: () => cb.current.fetchTags?.() ?? Promise.resolve([]),
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
    setActiveEditorView(view);
    loadedPath.current = path;
    // CAP-1: фокус в редактор при открытии заметки — пиши сразу, без клика (захват без трения).
    view.focus();
    // NAV-4: проскроллить к восстановленному курсору (selection при create не скроллит сам).
    if (initSelection) view.dispatch({ effects: EditorView.scrollIntoView(initSelection.anchor) });

    return () => {
      clearActiveEditorView(view);
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
    // NAV-4: уходя с заметки — запоминаем позицию курсора (в старом пути), входя — восстанавливаем
    // её (не прыгаем в начало длинной заметки). Watcher-reload того же файла (switching=false) курсор
    // сохраняет по-старому (anchor текущего view).
    if (switching) {
      useWorkspaceStore
        .getState()
        .setBufferCursor(loadedPath.current, view.state.selection.main.head);
    }
    loadedPath.current = path;
    const anchor = switching
      ? Math.min(useWorkspaceStore.getState().buffers[path]?.cursor ?? 0, initialDoc.length)
      : Math.min(view.state.selection.main.anchor, initialDoc.length);
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: initialDoc },
      selection: { anchor },
      annotations: externalSync.of(true),
      scrollIntoView: switching, // NAV-4: проскроллить к восстановленному курсору при смене файла
    });
    // CAP-1: смена файла фокусирует редактор (но НЕ watcher-reload того же файла — иначе крал бы
    // фокус у другой панели).
    if (switching) view.focus();
  }, [path, initialDoc]);

  return <div ref={host} className={styles.editor} data-testid="editor" />;
}
