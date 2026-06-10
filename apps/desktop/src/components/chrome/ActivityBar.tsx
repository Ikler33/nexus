import type { ReactNode } from 'react';
import { FileText, GitBranch, Home, Newspaper, Settings, Share2 } from 'lucide-react';
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
  const sidebarOpen = useUIStore((s) => s.sidebarOpen);
  const graphOpen = useUIStore((s) => s.graphOpen);
  const openHome = useUIStore((s) => s.openHome);
  const openNews = useUIStore((s) => s.openNews);
  const toggleSidebar = useUIStore((s) => s.toggleSidebar);
  const toggleGraph = useUIStore((s) => s.toggleGraph);
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
          <Newspaper size={19} aria-hidden />,
          t('commands.view.news'),
          openNews,
          newsOpen,
        )}
        {btn(
          <FileText size={19} aria-hidden />,
          t('sidebar.files'),
          toggleSidebar,
          !homeOpen && !newsOpen && sidebarOpen,
        )}
        {btn(
          <Share2 size={19} aria-hidden />,
          t('commands.view.graph'),
          toggleGraph,
          graphOpen,
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
