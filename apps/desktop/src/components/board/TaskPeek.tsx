import { useEffect, useState } from 'react';
import { CalendarClock, FolderClosed, SquarePen, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { MarkdownPreview } from '../editor/MarkdownPreview';
import { tauriApi, type TaskCard } from '../../lib/tauri-api';
import { basename, isOverdue, knownPriority, stripFrontmatter, todayIsoLocal } from './board-model';
import styles from './TaskPeek.module.css';

/**
 * Превью задачи (BOARD-6, спека §9): side-panel по клику на карточку — рендер ТЕЛА заметки
 * (`MarkdownPreview`, frontmatter срезан) + сводка свойств. НЕ модалка-трап: доска видна рядом.
 * Инлайн-правка свойств — PROP-3; здесь read-only + «Открыть в редакторе» для полного.
 */
export function TaskPeek({
  card,
  onClose,
  onOpenFull,
  onOpenLink,
}: {
  card: TaskCard;
  onClose: () => void;
  /** «Открыть в редакторе» — реальный путь карточки. */
  onOpenFull: (path: string) => void;
  /** Клик по `[[вики-ссылке]]` в теле — цель резолвится отдельно (openLink). */
  onOpenLink: (target: string) => void;
}) {
  const { t } = useTranslation();
  const [body, setBody] = useState<string | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    let alive = true;
    setBody(null);
    setError(false);
    tauriApi.vault
      .readFileMeta(card.path)
      .then((meta) => {
        if (alive) setBody(stripFrontmatter(meta.content));
      })
      .catch(() => {
        if (alive) setError(true);
      });
    return () => {
      alive = false;
    };
  }, [card.path]);

  const prio = knownPriority(card.priority);
  const overdue = isOverdue(card.due, todayIsoLocal());

  return (
    <aside className={styles.peek} aria-label={t('board.peek.title')}>
      <header className={styles.head}>
        <span className={styles.title}>{card.title || basename(card.path) || card.path}</span>
        <button
          type="button"
          className={styles.iconBtn}
          onClick={onClose}
          title={t('board.peek.close')}
          aria-label={t('board.peek.close')}
        >
          <X size={16} aria-hidden />
        </button>
      </header>

      <div className={styles.props}>
        <div className={styles.prop}>
          <span className={styles.propKey}>{t('board.peek.status')}</span>
          <span className={styles.propVal}>{card.status}</span>
        </div>
        {card.priority && (
          <div className={styles.prop}>
            <span className={styles.propKey}>{t('board.peek.priority')}</span>
            <span className={styles.propVal}>
              {prio ? t(`board.priority.${prio}`) : card.priority}
            </span>
          </div>
        )}
        {card.project && (
          <div className={styles.prop}>
            <span className={styles.propKey}>{t('board.peek.project')}</span>
            <span className={styles.propVal}>
              <FolderClosed size={12} aria-hidden /> {card.project}
            </span>
          </div>
        )}
        {card.due && (
          <div className={styles.prop}>
            <span className={styles.propKey}>{t('board.peek.due')}</span>
            <span className={`${styles.propVal} ${overdue ? styles.overdue : ''}`}>
              <CalendarClock size={12} aria-hidden /> {card.due}
              {overdue ? ` · ${t('board.overdue')}` : ''}
            </span>
          </div>
        )}
        {card.tags.length > 0 && (
          <div className={styles.tags}>
            {card.tags.map((tag) => (
              <span key={tag} className={styles.tag}>
                #{tag}
              </span>
            ))}
          </div>
        )}
      </div>

      <div className={styles.body}>
        {error ? (
          <p className={styles.muted}>{t('board.peek.error')}</p>
        ) : body === null ? (
          <p className={styles.muted}>{t('board.loading')}</p>
        ) : body.trim() === '' ? (
          <p className={styles.muted}>{t('board.peek.empty')}</p>
        ) : (
          <MarkdownPreview source={body} onOpenLink={(target) => onOpenLink(target)} />
        )}
      </div>

      <footer className={styles.foot}>
        <button type="button" className={styles.openBtn} onClick={() => onOpenFull(card.path)}>
          <SquarePen size={14} aria-hidden />
          {t('board.peek.openFull')}
        </button>
      </footer>
    </aside>
  );
}
