import { useCallback, useEffect, useState } from 'react';
import { ListChecks, RefreshCw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { getActiveEditorView } from '../../lib/editor/activeView';
import { collectTasks } from '../../lib/tasks/collect';
import { toggleTaskInPlace } from '../../lib/tasks/toggle';
import type { TaskItem } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { noteName, useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { BrandThinking } from '../chrome/BrandThinking';
import styles from './TasksPanel.module.css';

type Filter = 'open' | 'all';

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
  const hasVault = useVaultStore((s) => s.info != null);
  const [items, setItems] = useState<TaskItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<Filter>('open');

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
    if (ok) {
      // Оптимистично инвертируем (открытый фильтр → выполненная задача исчезнет из списка).
      setItems((prev) =>
        prev.map((x) =>
          x.path === item.path && x.line === item.line ? { ...x, checked: !x.checked } : x,
        ),
      );
    } else {
      void reload(); // строка уже не таск (дрейф между загрузкой и кликом) — перезагрузить
    }
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
  // Группировка по файлу с сохранением порядка появления.
  const groups: { path: string; title: string; tasks: TaskItem[] }[] = [];
  const byPath = new Map<string, number>();
  for (const task of visible) {
    let idx = byPath.get(task.path);
    if (idx == null) {
      idx = groups.length;
      byPath.set(task.path, idx);
      groups.push({ path: task.path, title: task.title ?? noteName(task.path), tasks: [] });
    }
    groups[idx].tasks.push(task);
  }

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
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
              <section key={g.path} className={styles.group}>
                <button
                  type="button"
                  className={styles.groupHead}
                  title={g.path}
                  onClick={() => openTaskLocation(g.path, g.tasks[0].line)}
                >
                  <span className={styles.groupTitle}>{g.title}</span>
                  <span className={styles.count}>{g.tasks.length}</span>
                </button>
                <ul className={styles.list}>
                  {g.tasks.map((task) => (
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
