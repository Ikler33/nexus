import { useEffect, useRef, useState } from 'react';
import { ChevronRight, Unlink } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi, type MentionEntry } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './MentionsBar.module.css';

// Дебаунс фонового рефреша при vault:changed (как в StatusBar/BacklinksBar).
const REFRESH_DEBOUNCE_MS = 1500;

/**
 * UNLINK-1: незалинкованные упоминания — заметки, чей ТЕКСТ содержит заголовок открытого файла, но
 * без явной `[[ссылки]]` (всплывание забытой связи, «Unlinked mentions» Obsidian). FTS-фраза по телу,
 * без уже-линкующих (бэкенд). Скрыт, если упоминаний нет (как OutlineBar — не шумит на типичной
 * заметке) и пока грузится. Клик ведёт к заметке-источнику.
 *
 * REFRESH: пере-запрашивает при `vault:changed` (новая заметка упомянула этот заголовок —
 * индексатор отработал). Фоновый рефреш «тихий»: НЕ обнуляет список на транзиентной ошибке (#296)
 * и не гасит срез (только смена `path` гасит сразу — урок гонки AIP-11/AIP-SQ).
 */
export function MentionsBar({ path }: { path: string }) {
  const { t } = useTranslation();
  const openFile = useWorkspaceStore((s) => s.openFile);
  const [open, setOpen] = useState(true);
  const [items, setItems] = useState<MentionEntry[]>([]);
  // Монотонный токен запроса: гонка «смена path + фоновый рефреш» — применяем только последний ответ.
  const reqRef = useRef(0);

  useEffect(() => {
    if (!path) {
      setItems([]);
      return;
    }
    let alive = true;
    setItems([]); // гасим срез прошлого файла СРАЗУ (урок гонки AIP-11/AIP-SQ)
    const run = (silent: boolean) => {
      const myReq = ++reqRef.current;
      tauriApi.graph
        .unlinkedMentions(path)
        .then((m) => {
          if (alive && myReq === reqRef.current) setItems(m);
        })
        .catch(() => {
          if (alive && myReq === reqRef.current && !silent) setItems([]);
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
