import { type CSSProperties, useCallback, useEffect, useMemo, useState } from 'react';
import {
  Clock,
  Command as CommandIcon,
  CornerDownLeft,
  FileText,
  Search,
  TextSearch,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { commands, type Command, formatCombo, spellCombo } from '../../lib/commands';
import { highlightTerms } from '../../lib/highlight';
import { tauriApi, type NoteRef, type SearchHit } from '../../lib/tauri-api';
import { usePrefsStore } from '../../stores/prefs';
import { useUIStore } from '../../stores/ui';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './CommandPalette.module.css';

/** Сколько файлов показываем в секции «Файлы» (DP-5, макет palette.jsx). */
const FILE_LIMIT = 8;
/** Сколько контент-результатов; min символов и debounce — гибрид дороже метаданных (бьёт эмбеддер). */
const CONTENT_LIMIT = 6;
const CONTENT_MIN_CHARS = 3;
const CONTENT_DEBOUNCE_MS = 250;

/** Сколько недавних заметок показываем на пустом запросе (NAV-2, ⌘O). */
const RECENTS_SHOWN = 8;

/** Строка результата: недавняя заметка, файл по метаданным, заметка по содержимому или команда. */
type Row =
  | { kind: 'recent'; path: string }
  | { kind: 'file'; note: NoteRef }
  | { kind: 'content'; hit: SearchHit }
  | { kind: 'command'; cmd: Command };

/** Заголовок строки из пути (basename без .md) — для недавних и файлов без title. */
function pathTitle(path: string): string {
  const base = path.slice(path.lastIndexOf('/') + 1);
  return base.endsWith('.md') ? base.slice(0, -3) : base;
}

function noteTitle(n: NoteRef): string {
  const base = n.path.slice(n.path.lastIndexOf('/') + 1);
  return n.title ?? (base.endsWith('.md') ? base.slice(0, -3) : base);
}

function hitTitle(h: SearchHit): string {
  const base = h.path.slice(h.path.lastIndexOf('/') + 1);
  return h.title ?? (base.endsWith('.md') ? base.slice(0, -3) : base);
}

/**
 * Command Palette (⌘K/⌘P) поверх единого реестра команд (§4.6). DP-5 (макет `palette.jsx`):
 * непустой запрос ищет И ФАЙЛЫ (search_vault, top-8), и команды — секции «Файлы»/«Команды»,
 * футер с клавиатурными хинтами. Клавиатура: ↑/↓ — выбор, Enter — выполнить/открыть, Esc.
 */
export function CommandPalette() {
  const open = useUIStore((s) => s.paletteOpen);
  const close = useUIStore((s) => s.closePalette);
  const paletteStyle = usePrefsStore((s) => s.paletteStyle);
  const { t } = useTranslation();

  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [version, setVersion] = useState(0);
  const [files, setFiles] = useState<NoteRef[]>([]);
  const [content, setContent] = useState<SearchHit[]>([]);
  const recents = useWorkspaceStore((s) => s.recents);

  useEffect(() => commands.subscribe(() => setVersion((v) => v + 1)), []);
  useEffect(() => {
    if (open) {
      setQuery('');
      setFiles([]);
      setContent([]);
      setActive(0);
    }
  }, [open]);

  const q = query.trim();
  // Поиск файлов с лёгким debounce (как сайдбар, Ф0-7); пустой запрос — без файловой секции.
  useEffect(() => {
    if (!open || !q) {
      setFiles([]);
      return;
    }
    let cancelled = false;
    const id = setTimeout(() => {
      tauriApi.search
        .searchVault(q)
        .then((r) => {
          if (!cancelled) setFiles(r.slice(0, FILE_LIMIT));
        })
        .catch(() => {
          if (!cancelled) setFiles([]);
        });
    }, 120);
    return () => {
      cancelled = true;
      clearTimeout(id);
    };
  }, [open, q]);

  // Контент-поиск по ТЕЛУ (NAV-1: закрывает «возврат сломан» — searchContent был без вызовов в UI).
  // Дороже метаданных (бьёт эмбеддер) → выше порог символов и дольше debounce.
  useEffect(() => {
    if (!open || q.length < CONTENT_MIN_CHARS) {
      setContent([]);
      return;
    }
    let cancelled = false;
    const id = setTimeout(() => {
      tauriApi.search
        .searchContent(q, { limit: CONTENT_LIMIT })
        .then((r) => {
          if (!cancelled) setContent(r);
        })
        .catch(() => {
          if (!cancelled) setContent([]);
        });
    }, CONTENT_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(id);
    };
  }, [open, q]);

  const label = useCallback((c: Command) => (c.titleKey ? t(c.titleKey) : c.title), [t]);

  const filteredCommands = useMemo(() => {
    void version; // пересчёт при register/dispose
    const needle = q.toLowerCase();
    const all = commands.list().sort((a, b) => label(a).localeCompare(label(b)));
    return needle ? all.filter((c) => label(c).toLowerCase().includes(needle)) : all;
  }, [q, version, label]);

  // Недавние (NAV-2): только на пустом запросе — быстрый возврат к последним заметкам (⌘O).
  const recentRows = useMemo(
    () => (q ? [] : recents.slice(0, RECENTS_SHOWN)),
    [q, recents],
  );

  const rows: Row[] = useMemo(
    () => [
      ...recentRows.map((path): Row => ({ kind: 'recent', path })),
      ...files.map((note): Row => ({ kind: 'file', note })),
      ...content.map((hit): Row => ({ kind: 'content', hit })),
      ...filteredCommands.map((cmd): Row => ({ kind: 'command', cmd })),
    ],
    [recentRows, files, content, filteredCommands],
  );

  if (!open) return null;

  const runAt = (index: number) => {
    const row = rows[index];
    if (!row) return;
    close();
    if (row.kind === 'recent') void useWorkspaceStore.getState().openFile(row.path);
    else if (row.kind === 'file') void useWorkspaceStore.getState().openFile(row.note.path);
    else if (row.kind === 'content') void useWorkspaceStore.getState().openFile(row.hit.path);
    else void commands.run(row.cmd.id);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setActive((a) => Math.min(rows.length - 1, a + 1));
        break;
      case 'ArrowUp':
        e.preventDefault();
        setActive((a) => Math.max(0, a - 1));
        break;
      case 'Enter':
        e.preventDefault();
        runAt(active);
        break;
      case 'Escape':
        e.preventDefault();
        close();
        break;
    }
  };

  const rowKey = (row: Row) =>
    row.kind === 'recent'
      ? `r:${row.path}`
      : row.kind === 'file'
        ? `f:${row.note.path}`
        : row.kind === 'content'
          ? `s:${row.hit.chunkId}`
          : `c:${row.cmd.id}`;

  const renderRow = (row: Row, i: number) => (
    <li
      key={rowKey(row)}
      role="option"
      aria-selected={i === active}
      data-active={i === active || undefined}
      className={styles.item}
      style={{ '--cmd-i': i } as CSSProperties}
      onMouseEnter={() => setActive(i)}
      onClick={() => runAt(i)}
    >
      {row.kind === 'recent' ? (
        <Clock size={15} className={styles.itemIco} aria-hidden />
      ) : row.kind === 'file' ? (
        <FileText size={15} className={styles.itemIco} aria-hidden />
      ) : row.kind === 'content' ? (
        <TextSearch size={15} className={styles.itemIco} aria-hidden />
      ) : (
        <CommandIcon size={15} className={styles.itemIco} aria-hidden />
      )}
      {row.kind === 'content' ? (
        <span className={styles.contentCell}>
          <span className={styles.title}>{hitTitle(row.hit)}</span>
          <span className={styles.snippet}>{highlightTerms(row.hit.snippet, q, styles.mark)}</span>
        </span>
      ) : (
        <span className={styles.title}>
          {row.kind === 'file'
            ? noteTitle(row.note)
            : row.kind === 'recent'
              ? pathTitle(row.path)
              : label(row.cmd)}
        </span>
      )}
      {row.kind === 'recent' ? (
        <span className={styles.hintPath}>{row.path}</span>
      ) : row.kind === 'file' ? (
        <span className={styles.hintPath}>{row.note.path}</span>
      ) : row.kind === 'command' ? (
        row.cmd.defaultKey && (
          <kbd className={styles.kbd} aria-label={spellCombo(row.cmd.defaultKey)}>
            {formatCombo(row.cmd.defaultKey)}
          </kbd>
        )
      ) : null}
    </li>
  );

  // Глобальные индексы секций (общая клавиатурная навигация по всем спискам).
  const nRecents = recentRows.length;
  const nFiles = files.length;
  const nContent = content.length;
  const fileStart = nRecents;
  const contentStart = nRecents + nFiles;
  const cmdStart = nRecents + nFiles + nContent;

  return (
    <div className={`${styles.overlay} ${styles[paletteStyle] ?? ''}`} onClick={close}>
      <div
        className={styles.palette}
        role="dialog"
        aria-label={t('palette.label')}
        onClick={(e) => e.stopPropagation()}
      >
        <div className={styles.inputRow}>
          <Search size={16} aria-hidden />
          <input
            className={styles.input}
            autoFocus
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            onKeyDown={onKeyDown}
            placeholder={t('palette.placeholder')}
            aria-label={t('palette.label')}
            role="combobox"
            aria-expanded
            aria-controls="command-list"
          />
          <kbd className={styles.kbd}>Esc</kbd>
        </div>
        <ul className={styles.list} id="command-list" role="listbox">
          {rows.length === 0 ? (
            <li className={styles.empty}>{t('palette.empty')}</li>
          ) : (
            <>
              {nRecents > 0 && (
                <li className={styles.section} aria-hidden>
                  {t('palette.recents')}
                </li>
              )}
              {rows.slice(0, nRecents).map((row, i) => renderRow(row, i))}
              {nFiles > 0 && (
                <li className={styles.section} aria-hidden>
                  {t('palette.files')}
                </li>
              )}
              {rows.slice(fileStart, fileStart + nFiles).map((row, i) => renderRow(row, fileStart + i))}
              {nContent > 0 && (
                <li className={styles.section} aria-hidden>
                  {t('palette.content')}
                </li>
              )}
              {rows
                .slice(contentStart, contentStart + nContent)
                .map((row, i) => renderRow(row, contentStart + i))}
              {filteredCommands.length > 0 && (
                <li className={styles.section} aria-hidden>
                  {t('palette.commands')}
                </li>
              )}
              {rows.slice(cmdStart).map((row, i) => renderRow(row, cmdStart + i))}
            </>
          )}
        </ul>
        <div className={styles.foot}>
          <span className={styles.footHint}>
            <kbd className={styles.kbd}>↑↓</kbd> {t('palette.navigate')}
          </span>
          <span className={styles.footHint}>
            <CornerDownLeft size={11} aria-hidden /> {t('palette.open')}
          </span>
        </div>
      </div>
    </div>
  );
}
