import { useEffect } from 'react';
import { RefreshCw, Target, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFocusTrap } from '../../hooks/useFocusTrap';
import { useGoalsStore } from '../../stores/goals';
import { BrandThinking } from '../chrome/BrandThinking';
import { useUIStore } from '../../stores/ui';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './GoalsPanel.module.css';

/**
 * Панель «Цели» (#35, vision-волна 2): vault-широкий дашборд заметок-целей (`#goal`) с прогресс-барами.
 * Модалка из титлбара. Прогресс 0–100 (бар) либо бейдж «нет прогресса» (D7); клик по цели открывает
 * заметку. Работает офлайн (чистый SQL-read). Загрузка при открытии + кнопка «Обновить».
 */
export function GoalsPanel() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeGoals);
  const trapRef = useFocusTrap<HTMLDivElement>(close);
  const items = useGoalsStore((s) => s.items);
  const loading = useGoalsStore((s) => s.loading);
  const load = useGoalsStore((s) => s.load);
  const openFile = useWorkspaceStore((s) => s.openFile);

  useEffect(() => {
    void load();
  }, [load]);

  const open = (path: string) => {
    close();
    void openFile(path);
  };

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        ref={trapRef}
        tabIndex={-1}
        className={styles.panel}
        role="dialog"
        aria-modal="true"
        aria-label={t('goals.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <span className={styles.iconBox}>
            <Target size={16} aria-hidden />
          </span>
          <span className={styles.title}>{t('goals.title')}</span>
          <span className={styles.spacer} />
          <button
            className={styles.iconBtn}
            onClick={() => void load()}
            title={t('goals.refresh')}
            aria-label={t('goals.refresh')}
          >
            <RefreshCw size={15} aria-hidden />
          </button>
          <button
            className={styles.iconBtn}
            onClick={close}
            title={t('goals.close')}
            aria-label={t('goals.close')}
          >
            <X size={15} aria-hidden />
          </button>
        </header>

        {loading ? (
          // Загрузка целей — «думающий» бренд-знак (макет insights.jsx).
          <div className={styles.thinking}>
            <BrandThinking size={26} />
            <span className="mt-label">{t('goals.loading')}</span>
          </div>
        ) : items.length === 0 ? (
          <div className={styles.emptyState}>
            <Target size={22} className={styles.emptyIco} aria-hidden />
            <p className={styles.empty}>{t('goals.empty')}</p>
          </div>
        ) : (
          <ul className={styles.list}>
            {items.map((g) => (
              <li key={g.path} className={styles.row}>
                <button
                  type="button"
                  className={styles.goalTitle}
                  title={g.path}
                  onClick={() => open(g.path)}
                >
                  {g.title ?? g.path}
                </button>
                {g.progress == null ? (
                  <span className={styles.noprog}>{t('goals.noProgress')}</span>
                ) : (
                  <div className={styles.barWrap}>
                    <div className={styles.bar}>
                      <div className={styles.fill} style={{ width: `${g.progress}%` }} />
                    </div>
                    <span className={styles.pct}>{g.progress}%</span>
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
