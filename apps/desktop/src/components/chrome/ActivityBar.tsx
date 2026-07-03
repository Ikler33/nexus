import type { ReactNode } from 'react';
import {
  CalendarCheck,
  FileText,
  GitBranch,
  Home,
  Inbox,
  LayoutGrid,
  ListChecks,
  Newspaper,
  Settings,
  Share2,
} from 'lucide-react';
import { CometIcon } from '../common/BrandGlyphs';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../stores/ui';
import styles from './ActivityBar.module.css';

/**
 * Вертикальный activity-bar на левом краю окна (DP-13, макет `app.jsx` `ActivityBar`,
 * Obsidian/VS Code-style): сверху навигация Home / Новости / Файлы (тоггл сайдбара) / Граф,
 * снизу Синхронизация (git) и Настройки. Сюда переехали входы, прежде жившие в титлбаре.
 */
export function ActivityBar() {
  const { t } = useTranslation();
  const homeOpen = useUIStore((s) => s.homeOpen);
  const newsOpen = useUIStore((s) => s.newsOpen);
  const boardOpen = useUIStore((s) => s.boardOpen);
  const todayOpen = useUIStore((s) => s.todayOpen);
  const agentOpen = useUIStore((s) => s.agentOpen);
  const sidebarOpen = useUIStore((s) => s.sidebarOpen);
  const graphOpen = useUIStore((s) => s.graphOpen);
  const tasksOpen = useUIStore((s) => s.tasksOpen);
  const inboxOpen = useUIStore((s) => s.inboxOpen);
  const openHome = useUIStore((s) => s.openHome);
  const openToday = useUIStore((s) => s.openToday);
  const openNews = useUIStore((s) => s.openNews);
  const openBoard = useUIStore((s) => s.openBoard);
  const openAgent = useUIStore((s) => s.openAgent);
  const toggleSidebar = useUIStore((s) => s.toggleSidebar);
  const toggleGraph = useUIStore((s) => s.toggleGraph);
  const toggleTasks = useUIStore((s) => s.toggleTasks);
  const toggleInbox = useUIStore((s) => s.toggleInbox);
  const toggleSync = useUIStore((s) => s.toggleSync);
  const openSettings = useUIStore((s) => s.openSettings);

  const btn = (icon: ReactNode, title: string, onClick: () => void, active = false) => (
    <button
      type="button"
      className={`${styles.actBtn} ${active ? styles.active : ''}`}
      onClick={onClick}
      title={title}
      aria-label={title}
      aria-pressed={active}
    >
      {icon}
    </button>
  );

  return (
    <nav className={styles.activityBar} aria-label={t('chrome.nav')}>
      <div className={styles.group}>
        {btn(<Home size={19} aria-hidden />, t('commands.view.home'), openHome, homeOpen)}
        {btn(
          <CalendarCheck size={19} aria-hidden />,
          t('commands.view.today'),
          openToday,
          todayOpen,
        )}
        {btn(
          <Newspaper size={19} aria-hidden />,
          t('commands.view.news'),
          openNews,
          newsOpen,
        )}
        {btn(
          <LayoutGrid size={19} aria-hidden />,
          t('commands.view.board'),
          openBoard,
          boardOpen,
        )}
        {/* P0-3-смоук: НЕ передавать openAgent голой ссылкой — onClick подставит MouseEvent в
            optional `seed`, и `seed.trim()` бросит TypeError (кнопка Castor «мертвела»). */}
        {btn(
          <CometIcon size={19} aria-hidden />,
          t('commands.view.agent'),
          () => openAgent(),
          agentOpen,
        )}
        {btn(
          <FileText size={19} aria-hidden />,
          t('sidebar.files'),
          toggleSidebar,
          !homeOpen && !newsOpen && !boardOpen && !todayOpen && !agentOpen && sidebarOpen,
        )}
        {btn(
          <Share2 size={19} aria-hidden />,
          t('commands.view.graph'),
          toggleGraph,
          graphOpen,
        )}
        {btn(
          <ListChecks size={19} aria-hidden />,
          t('commands.view.tasks'),
          toggleTasks,
          tasksOpen,
        )}
        {btn(
          <Inbox size={19} aria-hidden />,
          t('commands.view.inbox'),
          toggleInbox,
          inboxOpen,
        )}
      </div>
      <div className={styles.spacer} />
      <div className={styles.group}>
        {btn(<GitBranch size={19} aria-hidden />, t('commands.view.sync'), toggleSync)}
        {btn(
          <Settings size={19} aria-hidden />,
          t('commands.view.settings'),
          () => openSettings(),
        )}
      </div>
    </nav>
  );
}
