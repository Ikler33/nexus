import { useMemo, useState } from 'react';
import { ArrowDown, ArrowUp, CalendarClock, FolderClosed } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { TaskCard } from '../../lib/tauri-api';
import {
  cardTitle,
  filterTasks,
  isOverdue,
  knownPriority,
  normalizeStatus,
  priorityRank,
  type SortDir,
  type SortKey,
  sortTasks,
  type TaskFilter,
} from '../../lib/board/board-model';
import styles from './ListBoardView.module.css';

/** Распределённые из присутствующих карточек значения для фильтр-селектов (стейл-опций нет — только живые). */
interface FilterOptions {
  statuses: string[]; // raw-статусы (label через columnLabel)
  priorities: string[]; // raw-приоритеты, порядок по важности
  projects: string[];
  tags: string[];
}

/** Дедуп по нормализованному ключу с сохранением первого raw-значения. */
function distinct(raw: Iterable<string | null>, norm: (s: string) => string): string[] {
  const seen = new Map<string, string>();
  for (const v of raw) {
    if (v == null) continue;
    const trimmed = v.trim();
    if (!trimmed) continue;
    const key = norm(trimmed);
    if (!seen.has(key)) seen.set(key, trimmed);
  }
  return [...seen.values()];
}

const lower = (s: string) => s.toLowerCase();

interface ListBoardViewProps {
  cards: TaskCard[];
  today: string;
  onOpen: (path: string) => void;
  /** Локализация статуса (та же, что у колонок доски: дефолтные → i18n, кастом/переименованные → как есть). */
  columnLabel: (id: string) => string;
  /** Путь открытой в превью карточки — подсветка активной строки. */
  activePath?: string | null;
}

/** Класс цвета приоритета (известный набор → свой; прочее → нейтральный) — зеркалит BoardView. */
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
 * VIEW-1: плотный список задач — второе представление доски поверх ТЕХ ЖЕ карточек (read-only, без DnD и без
 * записи). Сортировка по клику на заголовок (title/status/priority/due), фильтры (статус/приоритет/проект/
 * тег/текст) — опции только из присутствующих карточек. Клик по строке открывает превью (TaskPeek) в
 * BoardView. Сорт/фильтр — чистые функции `sortTasks`/`filterTasks` (не мутируют общий массив карточек).
 */
export function ListBoardView({ cards, today, onOpen, columnLabel, activePath }: ListBoardViewProps) {
  const { t } = useTranslation();
  const [sortKey, setSortKey] = useState<SortKey>('due');
  const [sortDir, setSortDir] = useState<SortDir>('asc');
  const [filter, setFilter] = useState<TaskFilter>({});

  const options: FilterOptions = useMemo(() => {
    const statuses = distinct(
      cards.map((c) => c.status),
      normalizeStatus,
    ).sort((a, b) => columnLabel(a).localeCompare(columnLabel(b)));
    const priorities = distinct(
      cards.map((c) => c.priority),
      lower,
    ).sort((a, b) => priorityRank(a) - priorityRank(b) || a.localeCompare(b));
    const projects = distinct(
      cards.map((c) => c.project),
      lower,
    ).sort((a, b) => a.localeCompare(b));
    const tags = distinct(
      cards.flatMap((c) => c.tags),
      lower,
    ).sort((a, b) => a.localeCompare(b));
    return { statuses, priorities, projects, tags };
  }, [cards, columnLabel]);

  const rows = useMemo(
    () => sortTasks(filterTasks(cards, filter), sortKey, sortDir),
    [cards, filter, sortKey, sortDir],
  );

  const toggleSort = (key: SortKey) => {
    if (key === sortKey) setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'));
    else {
      setSortKey(key);
      setSortDir('asc');
    }
  };

  const patch = (p: Partial<TaskFilter>) => setFilter((f) => ({ ...f, ...p }));

  /** Заголовок сортируемой колонки: кнопка с меткой и стрелкой направления (если активна). */
  const SortHead = ({ col, label }: { col: SortKey; label: string }) => {
    const active = sortKey === col;
    return (
      <button
        type="button"
        className={`${styles.sortHead} ${active ? styles.sortActive : ''}`}
        onClick={() => toggleSort(col)}
        aria-sort={active ? (sortDir === 'asc' ? 'ascending' : 'descending') : 'none'}
      >
        {label}
        {active &&
          (sortDir === 'asc' ? (
            <ArrowUp size={12} aria-hidden />
          ) : (
            <ArrowDown size={12} aria-hidden />
          ))}
      </button>
    );
  };

  return (
    <div className={styles.listWrap}>
      <div className={styles.filterBar}>
        <select
          className={styles.filterSelect}
          value={filter.status ?? ''}
          onChange={(e) => patch({ status: e.target.value })}
          aria-label={t('board.list.col.status')}
        >
          <option value="">{t('board.list.filter.allStatuses')}</option>
          {options.statuses.map((s) => (
            <option key={s} value={s}>
              {columnLabel(s)}
            </option>
          ))}
        </select>
        <select
          className={styles.filterSelect}
          value={filter.priority ?? ''}
          onChange={(e) => patch({ priority: e.target.value })}
          aria-label={t('board.list.col.priority')}
        >
          <option value="">{t('board.list.filter.allPriorities')}</option>
          {options.priorities.map((p) => (
            <option key={p} value={p}>
              {knownPriority(p) ? t(`board.priority.${knownPriority(p)}`) : p}
            </option>
          ))}
        </select>
        {options.projects.length > 0 && (
          <select
            className={styles.filterSelect}
            value={filter.project ?? ''}
            onChange={(e) => patch({ project: e.target.value })}
            aria-label={t('board.list.col.project')}
          >
            <option value="">{t('board.list.filter.allProjects')}</option>
            {options.projects.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
        )}
        {options.tags.length > 0 && (
          <select
            className={styles.filterSelect}
            value={filter.tag ?? ''}
            onChange={(e) => patch({ tag: e.target.value })}
            aria-label={t('board.list.col.tags')}
          >
            <option value="">{t('board.list.filter.allTags')}</option>
            {options.tags.map((tg) => (
              <option key={tg} value={tg}>
                #{tg}
              </option>
            ))}
          </select>
        )}
        <input
          className={styles.filterText}
          type="text"
          value={filter.text ?? ''}
          onChange={(e) => patch({ text: e.target.value })}
          placeholder={t('board.list.filter.searchPlaceholder')}
          aria-label={t('board.list.filter.searchPlaceholder')}
        />
      </div>

      <div className={styles.listHead} role="row">
        <SortHead col="title" label={t('board.list.col.title')} />
        <SortHead col="status" label={t('board.list.col.status')} />
        <SortHead col="priority" label={t('board.list.col.priority')} />
        <SortHead col="due" label={t('board.list.col.due')} />
        <span className={styles.headPlain}>{t('board.list.col.project')}</span>
        <span className={styles.headPlain}>{t('board.list.col.tags')}</span>
      </div>

      {rows.length === 0 ? (
        <div className={styles.listEmpty}>{t('board.list.emptyFiltered')}</div>
      ) : (
        <div className={styles.listRows}>
          {rows.map((card) => {
            const overdue = isOverdue(card.due, today);
            const kp = knownPriority(card.priority);
            return (
              <button
                key={card.path}
                type="button"
                className={`${styles.row} ${activePath === card.path ? styles.rowActive : ''}`}
                onClick={() => onOpen(card.path)}
              >
                <span className={styles.cTitle}>{cardTitle(card)}</span>
                <span className={styles.cStatus}>{columnLabel(card.status)}</span>
                <span className={styles.cPrio}>
                  {card.priority ? (
                    <span className={`${styles.badge} ${prioClass(card.priority)}`}>
                      {kp ? t(`board.priority.${kp}`) : card.priority}
                    </span>
                  ) : (
                    <span className={styles.dash}>—</span>
                  )}
                </span>
                <span className={styles.cDue}>
                  {card.due ? (
                    <span className={`${styles.due} ${overdue ? styles.overdue : ''}`}>
                      <CalendarClock size={12} aria-hidden />
                      {card.due}
                      {overdue ? ` · ${t('board.overdue')}` : ''}
                    </span>
                  ) : (
                    <span className={styles.dash}>—</span>
                  )}
                </span>
                <span className={styles.cProject}>
                  {card.project ? (
                    <span className={styles.project}>
                      <FolderClosed size={12} aria-hidden />
                      {card.project}
                    </span>
                  ) : (
                    <span className={styles.dash}>—</span>
                  )}
                </span>
                <span className={styles.cTags}>
                  {card.tags.map((tg) => (
                    <span key={tg} className={styles.tag}>
                      #{tg}
                    </span>
                  ))}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
