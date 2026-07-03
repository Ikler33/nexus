import { useCallback, useEffect, useState } from 'react';
import { ListChecks, RefreshCw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { dateStamp } from '../../lib/daily';
import { useFocusTrap } from '../../hooks/useFocusTrap';
import { getActiveEditorView } from '../../lib/editor/activeView';
import { bucketOf, parseRecurrence, parseTaskMeta } from '../../lib/editor/format';
import { collectTasks } from '../../lib/tasks/collect';
import { toggleTaskInPlace } from '../../lib/tasks/toggle';
import type { TaskItem } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { noteName, useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { BrandThinking } from '../common/BrandThinking';
import styles from './TasksPanel.module.css';

type Filter = 'open' | 'all';
type GroupMode = 'date' | 'file';
/** Порядок временных бакетов в режиме группировки по дате (TASK-2). */
const BUCKETS = ['overdue', 'today', 'week', 'later', 'none'] as const;

/** Смещение начала 1-based строки `line` в тексте `doc` (для прыжка курсора + scrollIntoView). */
function lineToOffset(doc: string, line: number): number {
  const lines = doc.split('\n');
  let offset = 0;
  for (let i = 0; i < line - 1 && i < lines.length; i++) offset += lines[i].length + 1;
  return Math.min(offset, doc.length);
}

/**
 * Панель «Задачи» (TASK-1): сводка всех markdown-задач (`- [ ]`/`- [x]`) со всех заметок vault.
 * Скан на лету (collectTasks — буфер-aware: грязные буферы поверх диска). Клик по чекбоксу тогглит
 * исходную строку (toggleTaskInPlace — открытый буфер/диск); клик по тексту открывает заметку и
 * прыгает на строку. Фильтр открытые/все, группировка по файлу. Офлайн, строгий CSP (текст как узлы).
 */
export function TasksPanel() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeTasks);
  const trapRef = useFocusTrap<HTMLDivElement>(close);
  const hasVault = useVaultStore((s) => s.info != null);
  const [items, setItems] = useState<TaskItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<Filter>('open');
  const [groupMode, setGroupMode] = useState<GroupMode>('date');

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setItems(await collectTasks());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (hasVault) void reload();
  }, [hasVault, reload]);

  const onToggle = async (item: TaskItem) => {
    const ok = await toggleTaskInPlace(item.path, item.line);
    if (!ok) {
      void reload(); // строка уже не таск (дрейф между загрузкой и кликом) — перезагрузить
      return;
    }
    // RECUR-1: завершение повторяющейся задачи (🔁) порождает новую открытую копию с продвинутым
    // дедлайном — оптимистик-флип её не покажет, поэтому перезагружаем список целиком.
    if (!item.checked && parseRecurrence(item.text)) {
      void reload();
      return;
    }
    // Оптимистично инвертируем (открытый фильтр → выполненная задача исчезнет из списка).
    setItems((prev) =>
      prev.map((x) =>
        x.path === item.path && x.line === item.line ? { ...x, checked: !x.checked } : x,
      ),
    );
  };

  const openTaskLocation = (path: string, line: number) => {
    close();
    void useWorkspaceStore
      .getState()
      .openFile(path)
      .then(() => {
        // После того как редактор смонтировался/переключил path (его [path]-эффект отработал),
        // ставим курсор на строку задачи и скроллим. setTimeout(0) — после React-эффектов; работает
        // и для уже открытого файла (switching=false, где Editor сам не скроллит).
        setTimeout(() => {
          const view = getActiveEditorView();
          const buf = useWorkspaceStore.getState().buffers[path];
          if (!view || !buf) return;
          view.dispatch({ selection: { anchor: lineToOffset(buf.doc, line) }, scrollIntoView: true });
          view.focus();
        }, 0);
      });
  };

  const visible = filter === 'open' ? items.filter((x) => !x.checked) : items;
  // TASK-2: к каждой задаче прикрепляем мету (дедлайн/приоритет) и временной бакет (от сегодня).
  const today = dateStamp(new Date());
  const rows = visible.map((task) => {
    const meta = parseTaskMeta(task.text);
    return { task, meta, bucket: bucketOf(meta.due, today) };
  });
  type Row = (typeof rows)[number];
  interface Group {
    id: string;
    title: string;
    nav?: { path: string; line: number }; // клик по заголовку файла → навигация; у бакетов нет
    rows: Row[];
  }

  let groups: Group[] = [];
  if (groupMode === 'file') {
    const byPath = new Map<string, Group>();
    for (const r of rows) {
      let g = byPath.get(r.task.path);
      if (!g) {
        g = {
          id: r.task.path,
          title: r.task.title ?? noteName(r.task.path),
          nav: { path: r.task.path, line: r.task.line },
          rows: [],
        };
        byPath.set(r.task.path, g);
        groups.push(g);
      }
      g.rows.push(r);
    }
  } else {
    const byBucket = new Map<string, Group>(
      BUCKETS.map((b) => [b, { id: b, title: t(`tasks.bucket.${b}`), rows: [] }]),
    );
    for (const r of rows) byBucket.get(r.bucket)!.rows.push(r);
    // Внутри бакета: ближайший дедлайн выше, затем по приоритету (1 выше). Сентинел '9999-99-99'
    // для строк без даты — он надёжно сортируется в конец под localeCompare (ICU), в отличие от
    // пунктуации; на практике в одном бакете датированные и без-даты не смешиваются (защита-впрок).
    for (const g of byBucket.values()) {
      g.rows.sort(
        (a, z) =>
          (a.meta.due ?? '9999-99-99').localeCompare(z.meta.due ?? '9999-99-99') ||
          (a.meta.priority ?? 4) - (z.meta.priority ?? 4),
      );
    }
    groups = BUCKETS.map((b) => byBucket.get(b)!).filter((g) => g.rows.length > 0);
  }

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        ref={trapRef}
        tabIndex={-1}
        className={styles.panel}
        role="dialog"
        aria-modal="true"
        aria-label={t('tasks.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <ListChecks size={16} aria-hidden />
          <span className={styles.title}>{t('tasks.title')}</span>
          <span className={styles.spacer} />
          <div className={styles.filter} role="group" aria-label={t('tasks.title')}>
            <button
              className={filter === 'open' ? styles.filterActive : styles.filterBtn}
              onClick={() => setFilter('open')}
              aria-pressed={filter === 'open'}
            >
              {t('tasks.filterOpen')}
            </button>
            <button
              className={filter === 'all' ? styles.filterActive : styles.filterBtn}
              onClick={() => setFilter('all')}
              aria-pressed={filter === 'all'}
            >
              {t('tasks.filterAll')}
            </button>
          </div>
          <div className={styles.filter} role="group" aria-label={t('tasks.groupBy')}>
            <button
              className={groupMode === 'date' ? styles.filterActive : styles.filterBtn}
              onClick={() => setGroupMode('date')}
              aria-pressed={groupMode === 'date'}
            >
              {t('tasks.groupByDate')}
            </button>
            <button
              className={groupMode === 'file' ? styles.filterActive : styles.filterBtn}
              onClick={() => setGroupMode('file')}
              aria-pressed={groupMode === 'file'}
            >
              {t('tasks.groupByFile')}
            </button>
          </div>
          <button
            className={styles.iconBtn}
            onClick={() => void reload()}
            title={t('tasks.refresh')}
            aria-label={t('tasks.refresh')}
          >
            <RefreshCw size={15} aria-hidden />
          </button>
          <button
            className={styles.iconBtn}
            onClick={close}
            title={t('tasks.close')}
            aria-label={t('tasks.close')}
          >
            <X size={15} aria-hidden />
          </button>
        </header>

        {loading ? (
          <div className={styles.thinking}>
            <BrandThinking size={26} />
            <span className="mt-label">{t('tasks.loading')}</span>
          </div>
        ) : groups.length === 0 ? (
          <div className={styles.emptyState}>
            <ListChecks size={22} className={styles.emptyIco} aria-hidden />
            <p className={styles.empty}>
              {filter === 'open' ? t('tasks.emptyOpen') : t('tasks.empty')}
            </p>
          </div>
        ) : (
          <div className={styles.body}>
            {groups.map((g) => (
              <section key={g.id} className={styles.group}>
                {g.nav ? (
                  <button
                    type="button"
                    className={styles.groupHead}
                    title={g.id}
                    onClick={() => g.nav && openTaskLocation(g.nav.path, g.nav.line)}
                  >
                    <span className={styles.groupTitle}>{g.title}</span>
                    <span className={styles.count}>{g.rows.length}</span>
                  </button>
                ) : (
                  <div className={styles.bucketHead}>
                    <span className={styles.groupTitle}>{g.title}</span>
                    <span className={styles.count}>{g.rows.length}</span>
                  </div>
                )}
                <ul className={styles.list}>
                  {g.rows.map(({ task, meta, bucket }) => (
                    <li key={`${task.path}:${task.line}`} className={styles.row}>
                      <input
                        type="checkbox"
                        className={styles.checkbox}
                        checked={task.checked}
                        onChange={() => void onToggle(task)}
                        aria-label={task.text || t('tasks.untitled')}
                      />
                      <button
                        type="button"
                        className={task.checked ? styles.taskTextDone : styles.taskText}
                        onClick={() => openTaskLocation(task.path, task.line)}
                      >
                        {task.text || t('tasks.untitled')}
                      </button>
                      {meta.priority && (
                        <span className={`${styles.prio} ${styles[`prio${meta.priority}`]}`}>
                          P{meta.priority}
                        </span>
                      )}
                      {meta.due && (
                        <span
                          className={
                            bucket === 'overdue' ? `${styles.dueBadge} ${styles.dueOverdue}` : styles.dueBadge
                          }
                        >
                          {meta.due}
                        </span>
                      )}
                      {groupMode === 'date' && (
                        <span className={styles.fileHint} title={task.path}>
                          {task.title ?? noteName(task.path)}
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              </section>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
