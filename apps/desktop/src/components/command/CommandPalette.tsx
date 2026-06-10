import { type CSSProperties, useCallback, useEffect, useMemo, useState } from 'react';
import { Command as CommandIcon, CornerDownLeft, FileText, Search } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { commands, type Command, formatCombo } from '../../lib/commands';
import { tauriApi, type NoteRef } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './CommandPalette.module.css';

/** Сколько файлов показываем в секции «Файлы» (DP-5, макет palette.jsx). */
const FILE_LIMIT = 8;

/** Строка результата: файл vault или команда реестра. */
type Row = { kind: 'file'; note: NoteRef } | { kind: 'command'; cmd: Command };

function noteTitle(n: NoteRef): string {
  const base = n.path.slice(n.path.lastIndexOf('/') + 1);
  return n.title ?? (base.endsWith('.md') ? base.slice(0, -3) : base);
}

/**
 * Command Palette (⌘K/⌘P) поверх единого реестра команд (§4.6). DP-5 (макет `palette.jsx`):
 * непустой запрос ищет И ФАЙЛЫ (search_vault, top-8), и команды — секции «Файлы»/«Команды»,
 * футер с клавиатурными хинтами. Клавиатура: ↑/↓ — выбор, Enter — выполнить/открыть, Esc.
 */
export function CommandPalette() {
  const open = useUIStore((s) => s.paletteOpen);
  const close = useUIStore((s) => s.closePalette);
  const { t } = useTranslation();

  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [version, setVersion] = useState(0);
  const [files, setFiles] = useState<NoteRef[]>([]);

  useEffect(() => commands.subscribe(() => setVersion((v) => v + 1)), []);
  useEffect(() => {
    if (open) {
      setQuery('');
      setFiles([]);
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

  const label = useCallback((c: Command) => (c.titleKey ? t(c.titleKey) : c.title), [t]);

  const filteredCommands = useMemo(() => {
    void version; // пересчёт при register/dispose
    const needle = q.toLowerCase();
    const all = commands.list().sort((a, b) => label(a).localeCompare(label(b)));
    return needle ? all.filter((c) => label(c).toLowerCase().includes(needle)) : all;
  }, [q, version, label]);

  const rows: Row[] = useMemo(
    () => [
      ...files.map((note): Row => ({ kind: 'file', note })),
      ...filteredCommands.map((cmd): Row => ({ kind: 'command', cmd })),
    ],
    [files, filteredCommands],
  );

  if (!open) return null;

  const runAt = (index: number) => {
    const row = rows[index];
    if (!row) return;
    close();
    if (row.kind === 'file') void useWorkspaceStore.getState().openFile(row.note.path);
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

  const renderRow = (row: Row, i: number) => (
    <li
      key={row.kind === 'file' ? `f:${row.note.path}` : `c:${row.cmd.id}`}
      role="option"
      aria-selected={i === active}
      data-active={i === active || undefined}
      className={styles.item}
      style={{ '--cmd-i': i } as CSSProperties}
      onMouseEnter={() => setActive(i)}
      onClick={() => runAt(i)}
    >
      {row.kind === 'file' ? (
        <FileText size={15} className={styles.itemIco} aria-hidden />
      ) : (
        <CommandIcon size={15} className={styles.itemIco} aria-hidden />
      )}
      <span className={styles.title}>
        {row.kind === 'file' ? noteTitle(row.note) : label(row.cmd)}
      </span>
      {row.kind === 'file' ? (
        <span className={styles.hintPath}>{row.note.path}</span>
      ) : (
        row.cmd.defaultKey && <kbd className={styles.kbd}>{formatCombo(row.cmd.defaultKey)}</kbd>
      )}
    </li>
  );

  // Глобальные индексы секций (общая клавиатурная навигация по двум спискам).
  const fileRows = rows.filter((r) => r.kind === 'file');
  const cmdOffset = fileRows.length;

  return (
    <div className={styles.overlay} onClick={close}>
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
              {fileRows.length > 0 && (
                <li className={styles.section} aria-hidden>
                  {t('palette.files')}
                </li>
              )}
              {fileRows.map((row, i) => renderRow(row, i))}
              {filteredCommands.length > 0 && (
                <li className={styles.section} aria-hidden>
                  {t('palette.commands')}
                </li>
              )}
              {rows.slice(cmdOffset).map((row, i) => renderRow(row, cmdOffset + i))}
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
