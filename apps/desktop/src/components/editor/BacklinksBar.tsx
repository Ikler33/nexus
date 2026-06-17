import { useEffect, useRef, useState } from 'react';
import { ChevronRight, Link2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi, type BacklinkEntry } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './BacklinksBar.module.css';

// Дебаунс фонового рефреша при vault:changed (как в StatusBar): индексатор шлёт серию событий —
// схлопываем в один ре-запрос, не мигая UI.
const REFRESH_DEBOUNCE_MS = 1500;

/**
 * Backlinks-бар (DESIGN §3 editor-bottom): входящие ссылки файла `path` из SQLite (ADR-004).
 * Состояния loading / empty / список; клик ведёт к источнику. `path` — активная вкладка
 * своей группы (Б12). DP-3: шапка-твист сворачивает список (как в макете).
 *
 * REFRESH: пере-запрашивает при `vault:changed` (другая заметка добавила/убрала ссылку сюда —
 * индексатор отработал). Фоновый рефреш «тихий»: не дёргает loading и НЕ обнуляет список на
 * транзиентной ошибке (урок #296) — иначе бар мигал бы пустым на каждый чих воркера.
 */
export function BacklinksBar({ path }: { path: string }) {
  const { t } = useTranslation();
  const openFile = useWorkspaceStore((s) => s.openFile);

  const [loading, setLoading] = useState(false);
  const [open, setOpen] = useState(true);
  const [items, setItems] = useState<BacklinkEntry[]>([]);
  // Монотонный токен запроса: гонка «смена path + фоновый рефреш» — применяем только последний ответ.
  const reqRef = useRef(0);

  useEffect(() => {
    if (!path) {
      setItems([]);
      return;
    }
    let alive = true;
    const run = (silent: boolean) => {
      const myReq = ++reqRef.current;
      if (!silent) setLoading(true);
      tauriApi.graph
        .getBacklinks(path)
        .then((b) => {
          if (alive && myReq === reqRef.current) setItems(b);
        })
        .catch(() => {
          if (alive && myReq === reqRef.current && !silent) setItems([]);
        })
        .finally(() => {
          if (alive && myReq === reqRef.current && !silent) setLoading(false);
        });
    };
    run(false);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const debounced = () => {
      clearTimeout(timer);
      timer = setTimeout(() => run(true), REFRESH_DEBOUNCE_MS);
    };
    let off = () => {};
    void tauriApi.events.onVaultChanged(debounced).then((fn) => {
      off = fn;
    });
    return () => {
      alive = false;
      clearTimeout(timer);
      off();
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
                {/* DP-15 (макет): title заметки-источника, не путь с .md. */}
                <span className={styles.itemPath}>
                  {b.sourceTitle ?? b.sourcePath.replace(/\.md$/, '')}
                </span>
                {b.context && <span className={styles.itemContext}>{b.context}</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
