import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi } from '../../lib/tauri-api';
import type { DeadJob } from '../../lib/tauri-api';
import { relTime } from '../../lib/time';
import { useJobsStore } from '../../stores/jobs';
import styles from './DeadJobsModal.module.css';

/** Человеческие имена известных kind (ключи i18n). Динамический `t(kind)` нельзя:
 *  `home_widget:<имя>` содержит `:` — для i18next это разделитель namespace. */
const KIND_KEYS: Record<string, string> = {
  digest: 'deadJobs.kind.digest',
  contradictions: 'deadJobs.kind.contradictions',
  stale_radar: 'deadJobs.kind.staleRadar',
  newsfeed: 'deadJobs.kind.newsfeed',
  gc: 'deadJobs.kind.gc',
};
const HOME_WIDGET_PREFIX = 'home_widget:';

type Phase = { kind: 'loading' } | { kind: 'list'; jobs: DeadJob[] } | { kind: 'error'; message: string };

/**
 * Модалка за «⚠ N» статусбара (ADR-007 S7: dead-джобы не только видимы счётчиком, но и разбираемы):
 * список упавших фоновых задач — какая, почему (`last_error`), сколько попыток, когда; «Повторить»
 * (после исправления причины, напр. URL модели в Настройках) и «Очистить все» (видел, чинить не буду).
 */
export function DeadJobsModal({ onClose }: { onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      const jobs = await tauriApi.scheduler.deadJobs();
      setPhase({ kind: 'list', jobs });
    } catch (e) {
      setPhase({ kind: 'error', message: String(e) });
    }
    // Счётчик «⚠ N» в статусбаре — из стора; после retry/clear обновляем его сразу,
    // не дожидаясь события jobs:changed с воркера.
    void useJobsStore.getState().refresh();
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const kindLabel = (kind: string): string => {
    if (kind.startsWith(HOME_WIDGET_PREFIX))
      return `${t('deadJobs.kind.homeWidget')} · ${kind.slice(HOME_WIDGET_PREFIX.length)}`;
    const key = KIND_KEYS[kind];
    return key ? t(key) : kind;
  };

  const retry = async (id: number) => {
    setBusy(true);
    try {
      await tauriApi.scheduler.retryDead(id);
      await reload();
    } finally {
      setBusy(false);
    }
  };

  const clearAll = async () => {
    setBusy(true);
    try {
      await tauriApi.scheduler.clearDead();
      await reload();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.backdrop} onClick={onClose} role="presentation">
      <div
        className={styles.dialog}
        role="dialog"
        aria-label={t('deadJobs.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          <AlertTriangle size={15} className={styles.headIco} aria-hidden />
          <span className={styles.title}>{t('deadJobs.title')}</span>
          {phase.kind === 'list' && phase.jobs.length > 0 && (
            <button type="button" className={styles.clearBtn} disabled={busy} onClick={() => void clearAll()}>
              {t('deadJobs.clearAll')}
            </button>
          )}
          <button type="button" className={styles.close} onClick={onClose} aria-label={t('git.close')}>
            ✕
          </button>
        </header>
        <div className={styles.body}>
          {phase.kind === 'loading' && <p className={styles.muted}>{t('git.loading')}</p>}
          {phase.kind === 'error' && <p className={styles.errorMsg}>✋ {phase.message}</p>}
          {phase.kind === 'list' && phase.jobs.length === 0 && (
            <p className={styles.muted}>✓ {t('deadJobs.empty')}</p>
          )}
          {phase.kind === 'list' && phase.jobs.length > 0 && (
            <>
              <p className={styles.muted}>{t('deadJobs.hint')}</p>
              {phase.jobs.map((j) => (
                <section key={j.id} className={styles.job}>
                  <div className={styles.jobHead}>
                    <span className={styles.kind}>{kindLabel(j.kind)}</span>
                    <span className={styles.meta}>
                      {t('deadJobs.attempts', { count: j.attempts })} · {relTime(j.updatedAt, i18n.language)}
                    </span>
                    <button
                      type="button"
                      className={styles.retryBtn}
                      disabled={busy}
                      onClick={() => void retry(j.id)}
                    >
                      <RotateCcw size={12} aria-hidden />
                      {t('deadJobs.retry')}
                    </button>
                  </div>
                  <p className={styles.error}>{j.lastError ?? t('deadJobs.noError')}</p>
                </section>
              ))}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
