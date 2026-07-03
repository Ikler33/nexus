import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AlertTriangle,
  CalendarCheck,
  CalendarClock,
  FileText,
  FolderClosed,
  Inbox as InboxIcon,
  ListChecks,
  MessageSquare,
  PenLine,
  RefreshCw,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { dailyNotePath, dateStamp, INBOX, openOrCreateDaily } from '../../lib/daily';
import { parseInbox } from '../../lib/inbox/parse';
import { collectTasks } from '../../lib/tasks/collect';
import { bucketOf, parseTaskMeta } from '../../lib/editor/format';
import { tauriApi, type EpisodeRow, type TaskCard, type TaskItem } from '../../lib/tauri-api';
import { useChatStore } from '../../stores/chat';
import { useUIStore } from '../../stores/ui';
import { useWorkspaceStore } from '../../stores/workspace';
import {
  cardTitle,
  isOverdue,
  knownPriority,
  sortTasks,
  stripFrontmatter,
  todayIsoLocal,
} from '../../lib/board/board-model';
import styles from './TodayView.module.css';

interface BoardBuckets {
  overdue: TaskCard[];
  due: TaskCard[];
}
interface DailyState {
  exists: boolean;
  body: string;
}

const DAILY_PREVIEW_LINES = 12;
const EPISODE_LIMIT = 3;

/**
 * «Сегодня» (TODAY-1): утренний экран — сводка дня из УЖЕ существующих данных (read-only компоновка, без
 * нового бэкенда/LLM/egress). Пять секций: задачи доски (просрочено+сегодня по `due`), чек-задачи заметок
 * (`bucketOf`), заметка дня (превью тела, БЕЗ авто-создания), счётчик Входящих, недавние эпизоды. Каждая
 * секция fail-safe → пустое состояние при ошибке загрузки. Клики переиспользуют существующие переходы.
 */
export function TodayView() {
  const { t, i18n } = useTranslation();
  const closeToday = useUIStore((s) => s.closeToday);
  const toggleInbox = useUIStore((s) => s.toggleInbox);
  const openChat = useUIStore((s) => s.openChat);
  const loadSession = useChatStore((s) => s.loadSession);

  const [board, setBoard] = useState<BoardBuckets>({ overdue: [], due: [] });
  const [checklist, setChecklist] = useState<TaskItem[]>([]);
  const [daily, setDaily] = useState<DailyState>({ exists: false, body: '' });
  const [inboxCount, setInboxCount] = useState(0);
  const [episodes, setEpisodes] = useState<EpisodeRow[]>([]);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    const today = todayIsoLocal();

    // Каждый источник изолирован: его сбой даёт пустую секцию, а не валит весь экран (fail-safe).
    const boardP: Promise<BoardBuckets> = tauriApi.board
      .list()
      .then((cards) => {
        const sorted = sortTasks(cards, 'due', 'asc');
        return {
          overdue: sorted.filter((c) => isOverdue(c.due, today)),
          due: sorted.filter((c) => c.due === today),
        };
      })
      .catch(() => ({ overdue: [], due: [] }));

    const checklistP: Promise<TaskItem[]> = collectTasks()
      .then((tasks) =>
        tasks.filter((task) => {
          if (task.checked) return false;
          const b = bucketOf(parseTaskMeta(task.text).due, today);
          return b === 'overdue' || b === 'today';
        }),
      )
      .catch(() => []);

    // Заметка дня — СУЩЕСТВОВАНИЕ через file_hash (→ null = нет файла), НЕ авто-создаём (read-only).
    const dailyPath = dailyNotePath(new Date());
    const dailyP: Promise<DailyState> = tauriApi.vault
      .fileHash(dailyPath)
      .then(async (hash) => {
        if (hash == null) return { exists: false, body: '' };
        const content = await tauriApi.vault.readFile(dailyPath);
        return { exists: true, body: stripFrontmatter(content).trim() };
      })
      .catch(() => ({ exists: false, body: '' }));

    const inboxP: Promise<number> = tauriApi.vault
      .fileHash(INBOX)
      .then(async (hash) => (hash == null ? 0 : parseInbox(await tauriApi.vault.readFile(INBOX)).length))
      .catch(() => 0);

    const episodesP: Promise<EpisodeRow[]> = tauriApi.episode
      .list()
      .then((eps) => eps.filter((e) => !e.dismissed).slice(0, EPISODE_LIMIT))
      .catch(() => []);

    const [b, c, d, inbox, eps] = await Promise.all([boardP, checklistP, dailyP, inboxP, episodesP]);
    setBoard(b);
    setChecklist(c);
    setDaily(d);
    setInboxCount(inbox);
    setEpisodes(eps);
    setLoading(false);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const dateLabel = useMemo(() => {
    try {
      return new Date().toLocaleDateString(i18n.language === 'ru' ? 'ru-RU' : 'en-US', {
        weekday: 'long',
        day: 'numeric',
        month: 'long',
      });
    } catch {
      return dateStamp(new Date());
    }
  }, [i18n.language]);

  const openNote = (path: string) => {
    void useWorkspaceStore.getState().openFile(path);
    closeToday();
  };
  const openDaily = async () => {
    // Явное создание заметки дня — ТОЛЬКО по клику пользователя (не на рендере). Затем уходим в редактор.
    await openOrCreateDaily();
    closeToday();
  };
  const openEpisode = (sessionId: number) => {
    void loadSession(sessionId);
    openChat();
  };

  const dailyPreview = useMemo(
    () => daily.body.split('\n').slice(0, DAILY_PREVIEW_LINES).join('\n'),
    [daily.body],
  );
  const boardCount = board.overdue.length + board.due.length;
  const today = todayIsoLocal(); // один раз на рендер (не пересчитываем per-row в бейджах чек-задач)

  const boardRow = (card: TaskCard, kind: 'overdue' | 'due') => {
    const kp = knownPriority(card.priority);
    return (
      <button key={card.path} type="button" className={styles.item} onClick={() => openNote(card.path)}>
        <span className={styles.itemTitle}>{cardTitle(card)}</span>
        <span className={styles.itemMeta}>
          <span className={`${styles.badge} ${kind === 'overdue' ? styles.overdue : styles.due}`}>
            <CalendarClock size={11} aria-hidden />
            {card.due} · {t(`today.${kind === 'overdue' ? 'overdue' : 'due'}`)}
          </span>
          {kp && (
            <span
              className={`${styles.prio} ${kp === 'urgent' ? styles.prioUrgent : kp === 'high' ? styles.prioHigh : kp === 'medium' ? styles.prioMedium : styles.prioLow}`}
            >
              {t(`board.priority.${kp}`)}
            </span>
          )}
          {card.project && (
            <span className={styles.project}>
              <FolderClosed size={11} aria-hidden />
              {card.project}
            </span>
          )}
        </span>
      </button>
    );
  };

  return (
    <div className={styles.today}>
      <header className={styles.head}>
        <div className={styles.titleWrap}>
          <CalendarCheck size={20} aria-hidden />
          <h1 className={styles.title}>{t('today.title')}</h1>
          <span className={styles.date}>{dateLabel}</span>
        </div>
        <button
          type="button"
          className={styles.refresh}
          onClick={() => void load()}
          title={t('today.refresh')}
          aria-label={t('today.refresh')}
          disabled={loading}
        >
          <RefreshCw size={15} className={loading ? styles.spin : ''} aria-hidden />
        </button>
      </header>

      <div className={styles.scroll}>
        {/* Задачи доски: просрочено + сегодня (тот же sortTasks/isOverdue, что у VIEW-1 — без дрейфа). */}
        <section className={styles.section} aria-label={t('today.boardTasks')}>
          <div className={styles.sectionHead}>
            <ListChecks size={14} aria-hidden />
            {t('today.boardTasks')}
            {boardCount > 0 && <span className={styles.count}>{boardCount}</span>}
          </div>
          {boardCount === 0 ? (
            <div className={styles.empty}>{t('today.boardEmpty')}</div>
          ) : (
            <div className={styles.list}>
              {board.overdue.map((c) => boardRow(c, 'overdue'))}
              {board.due.map((c) => boardRow(c, 'due'))}
            </div>
          )}
        </section>

        {/* Чек-задачи в заметках: просрочено + сегодня (collectTasks + bucketOf). */}
        <section className={styles.section} aria-label={t('today.checklist')}>
          <div className={styles.sectionHead}>
            <CalendarClock size={14} aria-hidden />
            {t('today.checklist')}
            {checklist.length > 0 && <span className={styles.count}>{checklist.length}</span>}
          </div>
          {checklist.length === 0 ? (
            <div className={styles.empty}>{t('today.checklistEmpty')}</div>
          ) : (
            <div className={styles.list}>
              {checklist.map((task) => {
                const overdue = bucketOf(parseTaskMeta(task.text).due, today) === 'overdue';
                return (
                  <button
                    key={`${task.path}:${task.line}`}
                    type="button"
                    className={styles.item}
                    onClick={() => openNote(task.path)}
                  >
                    <span className={styles.itemTitle}>{task.text}</span>
                    <span className={styles.itemMeta}>
                      {overdue && (
                        <span className={`${styles.badge} ${styles.overdue}`}>
                          <AlertTriangle size={11} aria-hidden />
                          {t('today.overdue')}
                        </span>
                      )}
                      <span className={styles.project}>
                        <FileText size={11} aria-hidden />
                        {task.title || task.path}
                      </span>
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </section>

        {/* Заметка дня: превью тела, если файл есть; иначе пусто + кнопка создать (явный клик). */}
        <section className={styles.section} aria-label={t('today.dailyNote')}>
          <div className={styles.sectionHead}>
            <PenLine size={14} aria-hidden />
            {t('today.dailyNote')}
          </div>
          {daily.exists ? (
            <button
              type="button"
              className={styles.dailyCard}
              onClick={() => openNote(dailyNotePath(new Date()))}
            >
              <pre className={styles.dailyBody}>{dailyPreview || t('today.dailyEmpty')}</pre>
            </button>
          ) : (
            <div className={styles.emptyAction}>
              <span className={styles.empty}>{t('today.dailyEmpty')}</span>
              <button type="button" className={styles.actionBtn} onClick={() => void openDaily()}>
                <PenLine size={13} aria-hidden />
                {t('today.dailyCreate')}
              </button>
            </div>
          )}
        </section>

        {/* Входящие: счётчик quick-capture + переход в разбор (GTD-панель). */}
        <section className={styles.section} aria-label={t('today.inbox')}>
          <div className={styles.sectionHead}>
            <InboxIcon size={14} aria-hidden />
            {t('today.inbox')}
          </div>
          {inboxCount === 0 ? (
            <div className={styles.empty}>{t('today.inboxEmpty')}</div>
          ) : (
            <div className={styles.emptyAction}>
              <span className={styles.inboxCount}>{t('today.inboxCount', { count: inboxCount })}</span>
              <button type="button" className={styles.actionBtn} onClick={() => toggleInbox()}>
                <InboxIcon size={13} aria-hidden />
                {t('today.inboxOpen')}
              </button>
            </div>
          )}
        </section>

        {/* Недавние сессии: эпизоды (саммари); клик грузит сессию в чат. */}
        <section className={styles.section} aria-label={t('today.episodes')}>
          <div className={styles.sectionHead}>
            <MessageSquare size={14} aria-hidden />
            {t('today.episodes')}
          </div>
          {episodes.length === 0 ? (
            <div className={styles.empty}>{t('today.episodesEmpty')}</div>
          ) : (
            <div className={styles.list}>
              {episodes.map((e) => (
                <button
                  key={e.id}
                  type="button"
                  className={styles.item}
                  onClick={() => openEpisode(e.sessionId)}
                >
                  <span className={styles.itemTitle}>{e.sessionTitle}</span>
                  <span className={styles.episodeSummary}>{e.summary}</span>
                </button>
              ))}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
