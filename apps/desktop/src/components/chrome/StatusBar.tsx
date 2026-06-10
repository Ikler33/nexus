import { useEffect } from 'react';
import { HardDrive } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi } from '../../lib/tauri-api';
import { useJobsStore } from '../../stores/jobs';
import { useThemeStore } from '../../stores/theme';
import { useVaultStore } from '../../stores/vault';
import styles from './StatusBar.module.css';

/**
 * Нижний status bar (DP-4, макет app.jsx): слева статус-дот + путь vault, в центре —
 * индикатор фоновых задач (анимированный прогресс при работе планировщика, счётчики),
 * справа — Local · UTF-8 · Markdown. Счётчики джоб — по событию `jobs:changed` (ADR-007),
 * без поллинга. Git-конфликт-пилюля — после DP-10 (нужен дешёвый статус-канал, BACKLOG).
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
  const busy = running > 0 || pending > 0;
  const jobsTitle = t('status.jobsTitle', { running, pending, dead });

  return (
    <div className={styles.statusBar}>
      <span className={styles.item} title={info?.root}>
        <i className={`${styles.dot} ${dead > 0 ? styles.dotBad : styles.dotOk}`} aria-hidden />
        {info?.root ?? t('app.name')}
      </span>

      {busy && (
        <span className={`${styles.item} ${styles.jobs}`} title={jobsTitle}>
          <span className={styles.progress} aria-hidden>
            <i />
          </span>
          {t('status.working', { count: running + pending })}
        </span>
      )}
      {dead > 0 && (
        <span className={`${styles.item} ${styles.jobsDead}`} title={jobsTitle}>
          ⚠ {dead}
        </span>
      )}

      <div className={styles.right}>
        <span className={styles.item}>
          <HardDrive size={11} aria-hidden />
          {t('status.local')}
        </span>
        <span className={styles.item}>UTF-8</span>
        <span className={styles.item}>Markdown</span>
        <span className={styles.item}>{theme}</span>
      </div>
    </div>
  );
}
