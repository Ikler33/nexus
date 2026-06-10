import { useEffect, useState } from 'react';
import { ChevronRight, Link2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi, type BacklinkEntry } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './BacklinksBar.module.css';

/**
 * Backlinks-бар (DESIGN §3 editor-bottom): входящие ссылки файла `path` из SQLite (ADR-004).
 * Состояния loading / empty / список; клик ведёт к источнику. `path` — активная вкладка
 * своей группы (Б12). DP-3: шапка-твист сворачивает список (как в макете).
 */
export function BacklinksBar({ path }: { path: string }) {
  const { t } = useTranslation();
  const openFile = useWorkspaceStore((s) => s.openFile);

  const [loading, setLoading] = useState(false);
  const [open, setOpen] = useState(true);
  const [items, setItems] = useState<BacklinkEntry[]>([]);

  useEffect(() => {
    if (!path) {
      setItems([]);
      return;
    }
    let cancelled = false;
    setLoading(true);
    tauriApi.graph
      .getBacklinks(path)
      .then((b) => {
        if (!cancelled) setItems(b);
      })
      .catch(() => {
        if (!cancelled) setItems([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (!path) return null;

  return (
    <section className={styles.bar} aria-label={t('backlinks.title')}>
      <button
        type="button"
        className={styles.header}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <ChevronRight size={13} className={styles.twist} data-open={open || undefined} aria-hidden />
        <Link2 size={13} aria-hidden />
        <span>{items.length ? t('backlinks.count', { count: items.length }) : t('backlinks.title')}</span>
      </button>
      {!open ? null : loading ? (
        <p className={styles.state}>{t('backlinks.loading')}</p>
      ) : items.length === 0 ? (
        <p className={styles.state}>{t('backlinks.empty')}</p>
      ) : (
        <ul className={styles.list}>
          {items.map((b, i) => (
            <li key={`${b.sourcePath}:${b.lineNumber ?? 0}:${i}`}>
              <button className={styles.item} onClick={() => void openFile(b.sourcePath)}>
                <span className={styles.itemPath}>{b.sourcePath}</span>
                {b.context && <span className={styles.itemContext}>{b.context}</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
