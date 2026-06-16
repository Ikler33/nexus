import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, CalendarClock, FolderClosed, LayoutGrid, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi, type BoardData } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { useWorkspaceStore } from '../../stores/workspace';
import {
  applyOrder,
  basename,
  groupIntoColumns,
  isOverdue,
  knownPriority,
  OTHER_COLUMN_ID,
  todayIsoLocal,
} from './board-model';
import styles from './BoardView.module.css';

/** Дефолтные id колонок, для которых есть локализованная метка `board.col.*` (пустой label → локализуем). */
const LOCALIZED_COL_IDS = new Set(['todo', 'doing', 'done']);

/** Класс цвета приоритета (известный набор → свой; прочее → нейтральный). */
function prioClass(priority: string | null): string {
  switch (knownPriority(priority)) {
    case 'low':
      return styles.prioLow;
    case 'medium':
      return styles.prioMedium;
    case 'high':
      return styles.prioHigh;
    case 'urgent':
      return styles.prioUrgent;
    default:
      return styles.prioOther;
  }
}

/**
 * «Доска» (BOARD-4 + BOARD-3, спека `docs/specs/kanban-board.md`): канбан-вью заметок-задач. Колонки и
 * ручной порядок — из персист-конфига `.nexus/boards/<id>.json` (`get_board`); статусы вне набора колонок
 * → виртуальная «Прочее» (§12, задачи не теряются). DnD/реордер/редактор колонок — BOARD-5. Состояния:
 * загрузка, ошибка (последняя валидная доска цела, §14.6), битый конфиг (пилюля), пусто. Клик по карточке
 * открывает заметку (превью — BOARD-6). Refetch на фокус окна (`.nexus` невидим watcher).
 */
export function BoardView() {
  const { t } = useTranslation();
  const closeBoard = useUIStore((s) => s.closeBoard);
  const [data, setData] = useState<BoardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const next = await tauriApi.board.get();
      setData(next);
      setError(false);
    } catch {
      // Не обнуляем data — последняя валидная доска остаётся видимой (§14.6).
      setError(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // §14.6: `.nexus` невидим watcher'у → инвалидация доски на фокус окна (+ кнопка «Обновить»).
  useEffect(() => {
    const onFocus = () => void load();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [load]);

  const openNote = (path: string) => {
    void useWorkspaceStore.getState().openFile(path);
    closeBoard();
  };

  const today = todayIsoLocal();
  const total = data?.cards.length ?? 0;
  const config = data?.config;
  // Колонки из конфига; карточки — группировка по статусу + ручной порядок колонки.
  const columns = data
    ? groupIntoColumns(
        data.cards,
        config!.columns.map((c) => c.id),
      ).map((col) => ({ ...col, cards: applyOrder(col.cards, config!.order[col.id]) }))
    : [];
  const labelById = new Map((config?.columns ?? []).map((c) => [c.id, c.label]));
  const columnLabel = (id: string): string => {
    if (id === OTHER_COLUMN_ID) return t('board.col.other');
    const label = labelById.get(id);
    if (label) return label; // пользовательская метка (переименование без правки файлов)
    if (LOCALIZED_COL_IDS.has(id)) return t(`board.col.${id}`);
    return id; // кастомный id без метки — показываем как есть
  };

  return (
    <div className={styles.board}>
      <header className={styles.head}>
        <div className={styles.titleWrap}>
          <LayoutGrid size={20} aria-hidden />
          <h1 className={styles.title}>{t('board.title')}</h1>
          {data && (
            <span className={styles.total}>{t('board.taskCount', { count: total })}</span>
          )}
          {/* Битый JSON конфига (§14.6) — используется дефолт, файл НЕ перезаписан; видимый хинт. */}
          {data?.corrupt && (
            <span className={styles.errPill}>
              <AlertTriangle size={12} aria-hidden />
              {t('board.corruptConfig')}
            </span>
          )}
          {/* §14.6: ошибка ре-фетча при уже загруженной доске — последняя валидная доска цела, но
              провал виден (не молчит). Полноэкранная ошибка — только когда доски ещё нет. */}
          {error && data && (
            <span className={styles.errPill}>
              <AlertTriangle size={12} aria-hidden />
              {t('board.refreshError')}
            </span>
          )}
        </div>
        <button
          type="button"
          className={styles.refresh}
          onClick={() => void load()}
          title={t('board.refresh')}
          aria-label={t('board.refresh')}
          disabled={loading}
        >
          <RefreshCw size={15} className={loading ? styles.spin : ''} aria-hidden />
        </button>
      </header>

      {error && !data && (
        <div className={styles.state} role="alert">
          <AlertTriangle size={26} aria-hidden />
          <p>{t('board.loadError')}</p>
          <button type="button" className={styles.retry} onClick={() => void load()}>
            {t('board.retry')}
          </button>
        </div>
      )}

      {loading && !data && <div className={styles.state}>{t('board.loading')}</div>}

      {data && total === 0 && (
        <div className={styles.state}>
          <LayoutGrid size={30} aria-hidden />
          <p className={styles.emptyTitle}>{t('board.empty.title')}</p>
          <p className={styles.emptyBody}>{t('board.empty.body')}</p>
        </div>
      )}

      {data && total > 0 && (
        <div className={styles.columns}>
          {columns.map((col) => (
            <section key={col.id} className={styles.column} aria-label={columnLabel(col.id)}>
              <div className={styles.colHead}>
                <span className={styles.colTitle}>{columnLabel(col.id)}</span>
                <span className={styles.colCount}>{col.cards.length}</span>
              </div>
              <div className={styles.colCards}>
                {col.cards.map((card) => {
                  const overdue = isOverdue(card.due, today);
                  return (
                    <button
                      key={card.path}
                      type="button"
                      className={styles.card}
                      onClick={() => openNote(card.path)}
                    >
                      <span className={styles.cardTitle}>
                        {card.title || basename(card.path) || card.path}
                      </span>
                      {(card.priority || card.due || card.project) && (
                        <div className={styles.cardMeta}>
                          {card.priority && (
                            <span className={`${styles.badge} ${prioClass(card.priority)}`}>
                              {knownPriority(card.priority)
                                ? t(`board.priority.${knownPriority(card.priority)}`)
                                : card.priority}
                            </span>
                          )}
                          {card.due && (
                            <span className={`${styles.due} ${overdue ? styles.overdue : ''}`}>
                              <CalendarClock size={12} aria-hidden />
                              {card.due}
                              {overdue ? ` · ${t('board.overdue')}` : ''}
                            </span>
                          )}
                          {card.project && (
                            <span className={styles.project}>
                              <FolderClosed size={12} aria-hidden />
                              {card.project}
                            </span>
                          )}
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
                    </button>
                  );
                })}
                {col.cards.length === 0 && (
                  <div className={styles.colEmpty}>{t('board.colEmpty')}</div>
                )}
              </div>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}
