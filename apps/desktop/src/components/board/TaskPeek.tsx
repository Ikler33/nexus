import { useEffect, useState } from 'react';
import { SquarePen, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { MarkdownPreview } from '../editor/MarkdownPreview';
import { tauriApi, type TaskCard } from '../../lib/tauri-api';
import { basename, stripFrontmatter } from '../../lib/board/board-model';
import { PropertiesEditor } from './PropertiesEditor';
import styles from './TaskPeek.module.css';

/**
 * Превью задачи (BOARD-6 + PROP-3, спека §9/§7): side-panel по клику на карточку — Properties-панель
 * (типизированные виджеты + инлайн-правка через `set_frontmatter_field`) + рендер ТЕЛА заметки
 * (`MarkdownPreview`, frontmatter срезан) + теги. НЕ модалка-трап: доска видна рядом.
 */
export function TaskPeek({
  card,
  onClose,
  onOpenFull,
  onOpenLink,
  onChanged,
}: {
  card: TaskCard;
  onClose: () => void;
  /** «Открыть в редакторе» — реальный путь карточки. */
  onOpenFull: (path: string) => void;
  /** Клик по `[[вики-ссылке]]` в теле — цель резолвится отдельно (openLink). */
  onOpenLink: (target: string) => void;
  /** После инлайн-правки свойства (доска перечитывает карточки). */
  onChanged?: () => void;
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

      {/* PROP-3: типизированные свойства с инлайн-правкой (status/priority/due/…). */}
      <div className={styles.propsArea}>
        <PropertiesEditor
          key={card.path}
          path={card.path}
          onOpenSource={() => onOpenFull(card.path)}
          onChanged={onChanged}
        />
        {/* Теги — список (вне frontmatter-скаляров); инлайн-чип-правка — PROP-4. */}
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
