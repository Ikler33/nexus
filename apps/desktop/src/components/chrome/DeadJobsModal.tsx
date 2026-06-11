import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, ListTodo, RotateCcw, Power } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi } from '../../lib/tauri-api';
import type { ActiveJob, DeadJob } from '../../lib/tauri-api';
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

type Phase =
  | { kind: 'loading' }
  | { kind: 'list'; jobs: DeadJob[]; active: ActiveJob[] }
  | { kind: 'error'; message: string };

/**
 * Модалка фоновых задач за «N задач»/«⚠ N» статусбара (ADR-007 S7 + запрос владельца 2026-06-11
 * «посмотреть, какие джобы сейчас отрабатывают»): сверху очередь (выполняется/ждёт — какая задача
 * и когда готова), ниже ошибки — какая, почему (`last_error`), сколько попыток; «Повторить» (после
 * исправления причины, напр. URL модели в Настройках) и «Очистить все» (видел, чинить не буду).
 */
export function DeadJobsModal({ onClose }: { onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [busy, setBusy] = useState(false);
  const [restarting, setRestarting] = useState(false);

  // «через 507 мин» пугает (суточные джобы) → человеческие единицы: мин до 1.5 ч, часы до 1.5 сут, дни.
  const inLabel = (runAt: number): string => {
    const mins = Math.max(1, Math.ceil((runAt * 1000 - Date.now()) / 60_000));
    if (mins < 90) return t('deadJobs.inMin', { n: mins });
    const hours = Math.round(mins / 60);
    if (hours < 36) return t('deadJobs.inHours', { n: hours });
    return t('deadJobs.inDays', { n: Math.round(hours / 24) });
  };

  const restart = async () => {
    if (restarting) return;
    setRestarting(true);
    try {
      await tauriApi.scheduler.restart();
      // Дать новому воркеру тик заклеймить готовые джобы, затем перечитать.
      await new Promise((r) => setTimeout(r, 1500));
      await reload();
      void useJobsStore.getState().refresh();
    } finally {
      setRestarting(false);
    }
  };

  const reload = useCallback(async () => {
    try {
      const [jobs, active] = await Promise.all([
        tauriApi.scheduler.deadJobs(),
        tauriApi.scheduler.activeJobs(),
      ]);
      setPhase({ kind: 'list', jobs, active });
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
          {phase.kind === 'list' && (
            <button
              type="button"
              className={styles.clearBtn}
              disabled={busy || restarting}
              onClick={() => void restart()}
              title={t('deadJobs.restartHint')}
            >
              <Power size={12} aria-hidden /> {restarting ? t('deadJobs.restarting') : t('deadJobs.restart')}
            </button>
          )}
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
          {phase.kind === 'list' && phase.active.length === 0 && phase.jobs.length === 0 && (
            <p className={styles.muted}>✓ {t('deadJobs.empty')}</p>
          )}
          {phase.kind === 'list' && phase.active.length > 0 && (
            <>
              <p className={styles.section}>
                <ListTodo size={13} aria-hidden /> {t('deadJobs.queue')}
              </p>
              {phase.active.map((j) => (
                <section key={j.id} className={styles.job}>
                  <div className={styles.jobHead}>
                    <span className={styles.kind}>{kindLabel(j.kind)}</span>
                    <span className={styles.meta}>
                      {j.state === 'running'
                        ? t('deadJobs.running')
                        : j.runAt * 1000 <= Date.now()
                          ? t('deadJobs.queued')
                          : inLabel(j.runAt)}
                    </span>
                  </div>
                </section>
              ))}
            </>
          )}
          {phase.kind === 'list' && phase.jobs.length > 0 && (
            <>
              <p className={styles.section}>
                <AlertTriangle size={13} aria-hidden /> {t('deadJobs.errorsSection')}
              </p>
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
