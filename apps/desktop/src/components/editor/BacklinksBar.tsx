import { useEffect, useState } from 'react';
import { Link2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi, type BacklinkEntry } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './BacklinksBar.module.css';

/**
 * Backlinks-бар (DESIGN §3 editor-bottom): входящие ссылки файла `path` из SQLite (ADR-004).
 * Состояния loading / empty / список; клик ведёт к источнику. `path` — активная вкладка
 * своей группы (Б12).
 */
export function BacklinksBar({ path }: { path: string }) {
  const { t } = useTranslation();
  const openFile = useWorkspaceStore((s) => s.openFile);

  const [loading, setLoading] = useState(false);
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
      <header className={styles.header}>
        <Link2 size={13} aria-hidden />
        <span>{items.length ? t('backlinks.count', { count: items.length }) : t('backlinks.title')}</span>
      </header>
      {loading ? (
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
