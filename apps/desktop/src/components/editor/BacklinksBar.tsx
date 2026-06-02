import { useEffect, useState } from 'react';
import { Link2 } from 'lucide-react';
import { tauriApi, type BacklinkEntry } from '../../lib/tauri-api';
import { useVaultStore } from '../../stores/vault';
import styles from './BacklinksBar.module.css';

/**
 * Backlinks-бар (DESIGN §3 editor-bottom): входящие ссылки активного файла из SQLite
 * (ADR-004). Состояния loading / empty / список; клик ведёт к источнику.
 */
export function BacklinksBar() {
  const activeFile = useVaultStore((s) => s.activeFile);
  const openFile = useVaultStore((s) => s.openFile);
  const path = activeFile?.path ?? null;

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
    <section className={styles.bar} aria-label="Backlinks">
      <header className={styles.header}>
        <Link2 size={13} aria-hidden />
        <span>Беклинки{items.length ? ` · ${items.length}` : ''}</span>
      </header>
      {loading ? (
        <p className={styles.state}>Загрузка…</p>
      ) : items.length === 0 ? (
        <p className={styles.state}>Нет обратных ссылок</p>
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
