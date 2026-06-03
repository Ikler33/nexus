import { useTranslation } from 'react-i18next';
import { useThemeStore } from '../../stores/theme';
import { useVaultStore } from '../../stores/vault';
import styles from './StatusBar.module.css';

/**
 * Нижний status bar дизайн-системы. Пока — путь vault (слева) + индикатор темы (справа).
 * Богаче (провайдер модели, прогресс индексации, sync-статус) — отдельными срезами, по мере
 * проводки соответствующих данных (без фейковых значений).
 */
export function StatusBar() {
  const { t } = useTranslation();
  const info = useVaultStore((s) => s.info);
  const theme = useThemeStore((s) => s.theme);

  return (
    <div className={styles.statusBar}>
      <span className={styles.item} title={info?.root}>
        {info?.root ?? t('app.name')}
      </span>
      <div className={styles.right}>
        <span className={styles.item}>{theme === 'dark' ? 'dark' : 'light'}</span>
      </div>
    </div>
  );
}
