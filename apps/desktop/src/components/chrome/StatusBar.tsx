import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { tauriApi } from '../../lib/tauri-api';
import { useJobsStore } from '../../stores/jobs';
import { useThemeStore } from '../../stores/theme';
import { useVaultStore } from '../../stores/vault';
import styles from './StatusBar.module.css';

/**
 * Нижний status bar дизайн-системы: путь vault (слева) + индикатор фоновых задач планировщика и темы
 * (справа). Счётчики джоб обновляются по событию `jobs:changed` (ADR-007 срез 5) — без поллинга.
 */
export function StatusBar() {
  const { t } = useTranslation();
  const info = useVaultStore((s) => s.info);
  const theme = useThemeStore((s) => s.theme);
  const counts = useJobsStore((s) => s.counts);

  // Подписка на «очередь изменилась» + первичная загрузка (StatusBar монтируется при открытом vault).
  useEffect(() => {
    const refresh = () => void useJobsStore.getState().refresh();
    refresh();
    let unlisten = () => {};
    void tauriApi.events.onJobsChanged(refresh).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten();
  }, []);

  const { running, pending, dead } = counts;
  const showJobs = running > 0 || pending > 0 || dead > 0;
  const jobsTitle = t('status.jobsTitle', { running, pending, dead });

  return (
    <div className={styles.statusBar}>
      <span className={styles.item} title={info?.root}>
        {info?.root ?? t('app.name')}
      </span>
      <div className={styles.right}>
        {showJobs && (
          <span className={`${styles.item} ${styles.jobs}`} title={jobsTitle}>
            {running > 0 && (
              <span className={styles.jobsActive}>⚙ {running}</span>
            )}
            {pending > 0 && <span>⏳ {pending}</span>}
            {dead > 0 && <span className={styles.jobsDead}>⚠ {dead}</span>}
          </span>
        )}
        <span className={styles.item}>{theme === 'dark' ? 'dark' : 'light'}</span>
      </div>
    </div>
  );
}
