import { useEffect, useRef, useState } from 'react';
import { Link2, Plus } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { noteName } from '../../stores/vault';
import { type NoteRef } from '../../lib/tauri-api';
import styles from './AppendLine.module.css';

/**
 * AppendLine (макет editor.jsx): однострочный quick-add внизу превью. Enter → дописать строку в
 * конец заметки через `onAppend` (родитель пишет в буфер — БЕЗ нового бэкенда, как обычная правка).
 *
 * `[[` → автокомплит вики-ссылок: тот же паттерн, что у CM6-редактора (extensions.ts wikilinkSource) —
 * `fetchNotes(query)` спрашивает у бэкенда топ-N (`vault.listNotes`), выбор вставляет `[[Note]]`.
 * Поп-ап рисуем сами (как у MentionsBar/wl-pop макета), т.к. это plain `<input>`, не CodeMirror.
 *
 * Скрыт в режиме чтения (родитель не рендерит) — это инструмент правки. Стиль — global-search-инпут.
 */
export function AppendLine({
  onAppend,
  fetchNotes,
}: {
  /** Дописать одну строку в конец заметки (родитель пишет в буфер). */
  onAppend: (line: string) => void;
  /** Заметки по подстроке для автокомплита `[[…` — тот же источник, что у CM6 (#22). */
  fetchNotes: (query: string) => Promise<NoteRef[]>;
}) {
  const { t } = useTranslation();
  const [val, setVal] = useState('');
  // Поп-ап автокомплита `[[…`: `query` — набранный после `[[` префикс, `start` — позиция самого `[[`.
  const [pop, setPop] = useState<{ query: string; start: number } | null>(null);
  const [matches, setMatches] = useState<NoteRef[]>([]);
  const [sel, setSel] = useState(0);
  const ref = useRef<HTMLInputElement>(null);
  // Монотонный токен запроса: гонка «быстрый набор» — применяем только последний ответ (как BacklinksBar).
  const reqRef = useRef(0);

  // Асинхронно подтягиваем совпадения при изменении запроса (тот же fetchNotes, что у редактора).
  useEffect(() => {
    if (!pop) {
      setMatches([]);
      return;
    }
    let alive = true;
    const myReq = ++reqRef.current;
    fetchNotes(pop.query)
      .then((notes) => {
        if (alive && myReq === reqRef.current) {
          setMatches(notes.slice(0, 6));
          setSel(0);
        }
      })
      .catch(() => {
        if (alive && myReq === reqRef.current) setMatches([]);
      });
    return () => {
      alive = false;
    };
  }, [pop, fetchNotes]);

  /** Текст до курсора оканчивается на `[[<query>` (без закрытия) → открыть поп-ап с этим query. */
  function detect(text: string, caret: number | null) {
    const before = text.slice(0, caret ?? text.length);
    const m = /\[\[([^\]\n]*)$/.exec(before);
    if (m) setPop({ query: m[1], start: (caret ?? text.length) - m[1].length });
    else setPop(null);
  }

  /** Вставить `[[Title]]` вместо набранного `[[query`. */
  function pick(item: NoteRef) {
    if (!pop || !ref.current) return;
    const caret = ref.current.selectionStart ?? val.length;
    const head = val.slice(0, pop.start - 2); // отбрасываем `[[`
    const tail = val.slice(caret);
    const inserted = `[[${noteName(item.path)}]]`;
    const next = head + inserted + tail;
    setVal(next);
    setPop(null);
    const caretPos = (head + inserted).length;
    requestAnimationFrame(() => {
      ref.current?.focus();
      ref.current?.setSelectionRange(caretPos, caretPos);
    });
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (pop && matches.length) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSel((s) => (s + 1) % matches.length);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSel((s) => (s - 1 + matches.length) % matches.length);
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        pick(matches[sel]);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        setPop(null);
        return;
      }
    }
    if (e.key === 'Enter' && val.trim()) {
      onAppend(val.trim());
      setVal('');
      setPop(null);
    }
  }

  return (
    <div className={styles.wrap}>
      <div className={styles.field}>
        <Plus size={14} aria-hidden className={styles.icon} />
        <input
          ref={ref}
          className={styles.input}
          value={val}
          onChange={(e) => {
            setVal(e.target.value);
            detect(e.target.value, e.target.selectionStart);
          }}
          onKeyDown={onKeyDown}
          onClick={(e) => detect(val, e.currentTarget.selectionStart)}
          placeholder={t('editor.append.placeholder')}
          aria-label={t('editor.append.label')}
        />
      </div>
      {pop && matches.length > 0 && (
        <ul className={styles.pop} role="listbox" aria-label={t('editor.append.label')}>
          {matches.map((m, i) => (
            <li key={m.path}>
              <button
                type="button"
                className={`${styles.item} ${i === sel ? styles.sel : ''}`}
                role="option"
                aria-selected={i === sel}
                onMouseEnter={() => setSel(i)}
                // onMouseDown (не onClick): успеваем вставить до blur инпута (иначе поп-ап схлопнулся бы).
                onMouseDown={(e) => {
                  e.preventDefault();
                  pick(m);
                }}
              >
                <Link2 size={13} aria-hidden className={styles.itemIco} />
                <span className={styles.itemName}>{m.title ?? noteName(m.path)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
