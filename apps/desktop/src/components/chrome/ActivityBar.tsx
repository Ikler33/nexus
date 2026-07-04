import { Fragment, type ReactNode } from 'react';
import { FileText, GitBranch, Inbox, ListChecks, Settings, Share2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { viewRegistry } from '../../lib/connector';
import { selectMainView, useUIStore } from '../../stores/ui';
import styles from './ActivityBar.module.css';

/**
 * Вертикальный activity-bar на левом краю окна (DP-13, макет `app.jsx` `ActivityBar`,
 * Obsidian/VS Code-style): сверху навигация Home / Новости / Файлы (тоггл сайдбара) / Граф,
 * снизу Синхронизация (git) и Настройки. Сюда переехали входы, прежде жившие в титлбаре.
 *
 * F-8: кнопки main-вью (Home/Сегодня/Новости/Доска/Castor) теперь берутся из реестра `views`
 * коннектора (order/icon/titleKey/activate/isActive) — легализация прежнего хардкода. Не-вью входы
 * (файлы/граф/задачи/входящие/синх/настройки — это тогглы оверлеев, не main-вью) остаются здесь.
 */
export function ActivityBar() {
  const { t } = useTranslation();
  // F-4 (семейство 1): активная main-вью одним derived-селектором вместо 5 `*Open`-булей.
  const mainView = useUIStore(selectMainView);
  const sidebarOpen = useUIStore((s) => s.sidebarOpen);
  const graphOpen = useUIStore((s) => s.graphOpen);
  const tasksOpen = useUIStore((s) => s.tasksOpen);
  const inboxOpen = useUIStore((s) => s.inboxOpen);
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

  // Main-вью коннектора (F-8): порядок/иконка/имя/действие — из реестра `views`.
  const viewButtons = viewRegistry.list().filter((v) => v.activityBar);

  return (
    <nav className={styles.activityBar} aria-label={t('chrome.nav')}>
      <div className={styles.group}>
        {viewButtons.map((v) => (
          <Fragment key={v.id}>
            {btn(<v.icon size={19} aria-hidden />, t(v.titleKey), v.activate, v.isActive(mainView))}
          </Fragment>
        ))}
        {btn(
          <FileText size={19} aria-hidden />,
          t('sidebar.files'),
          toggleSidebar,
          mainView === 'editor' && sidebarOpen,
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
