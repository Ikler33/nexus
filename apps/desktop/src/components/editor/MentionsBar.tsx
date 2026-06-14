import { useEffect, useState } from 'react';
import { ChevronRight, Unlink } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi, type MentionEntry } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './MentionsBar.module.css';

/**
 * UNLINK-1: незалинкованные упоминания — заметки, чей ТЕКСТ содержит заголовок открытого файла, но
 * без явной `[[ссылки]]` (всплывание забытой связи, «Unlinked mentions» Obsidian). FTS-фраза по телу,
 * без уже-линкующих (бэкенд). Скрыт, если упоминаний нет (как OutlineBar — не шумит на типичной
 * заметке) и пока грузится. Клик ведёт к заметке-источнику.
 */
export function MentionsBar({ path }: { path: string }) {
  const { t } = useTranslation();
  const openFile = useWorkspaceStore((s) => s.openFile);
  const [open, setOpen] = useState(true);
  const [items, setItems] = useState<MentionEntry[]>([]);

  useEffect(() => {
    if (!path) {
      setItems([]);
      return;
    }
    let cancelled = false;
    setItems([]); // гасим срез прошлого файла СРАЗУ (урок гонки AIP-11/AIP-SQ)
    tauriApi.graph
      .unlinkedMentions(path)
      .then((m) => {
        if (!cancelled) setItems(m);
      })
      .catch(() => {
        if (!cancelled) setItems([]);
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (!path || items.length === 0) return null; // нет упоминаний / грузится → бар скрыт

  return (
    <section className={styles.bar} aria-label={t('mentions.title')}>
      <button
        type="button"
        className={styles.header}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <ChevronRight size={13} className={styles.twist} data-open={open || undefined} aria-hidden />
        <Unlink size={13} aria-hidden />
        <span>{t('mentions.count', { count: items.length })}</span>
      </button>
      {open && (
        <ul className={styles.list}>
          {items.map((m, i) => (
            <li key={`${m.sourcePath}:${i}`}>
              <button
                className={styles.item}
                onClick={() => void openFile(m.sourcePath)}
                title={m.sourcePath}
              >
                <span className={styles.itemPath}>
                  {m.sourceTitle ?? m.sourcePath.replace(/\.md$/, '')}
                </span>
                {m.snippet && <span className={styles.itemContext}>{m.snippet}</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
