import { useEffect } from 'react';
import { Newspaper, RefreshCw, Sparkles, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useDigestStore } from '../../stores/digest';
import { useUIStore } from '../../stores/ui';
import styles from './DigestPanel.module.css';

/** Unix-секунды → локальная дата-время (короткий формат). */
function fmt(ts: number, locale: string): string {
  return new Date(ts * 1000).toLocaleString(locale, {
    day: 'numeric',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/**
 * Панель «Дайджест изменений» (#35, ADR-007 slice 4): показывает последний LLM-дайджест недавно
 * изменённых заметок + кнопку «сгенерировать сейчас». Генерация асинхронна (джоба планировщика) —
 * результат прилетает по `jobs:changed` (refetch в App). Модалка из титлбара. Контент — простой
 * текст со списком (`pre-wrap`, без тяжёлого markdown-рендера).
 */
export function DigestPanel() {
  const { t, i18n } = useTranslation();
  const close = useUIStore((s) => s.closeDigest);
  const latest = useDigestStore((s) => s.latest);
  const loading = useDigestStore((s) => s.loading);
  const generating = useDigestStore((s) => s.generating);
  const error = useDigestStore((s) => s.error);
  const load = useDigestStore((s) => s.load);
  const generate = useDigestStore((s) => s.generate);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        className={styles.panel}
        role="dialog"
        aria-modal="true"
        aria-label={t('digest.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <Newspaper size={16} aria-hidden />
          <span className={styles.title}>{t('digest.title')}</span>
          <span className={styles.spacer} />
          <button
            className={styles.genBtn}
            onClick={() => void generate()}
            disabled={generating}
            title={t('digest.generate')}
          >
            <Sparkles size={14} aria-hidden />
            <span>{generating ? t('digest.generating') : t('digest.generate')}</span>
          </button>
          <button
            className={styles.iconBtn}
            onClick={() => void load()}
            title={t('digest.refresh')}
            aria-label={t('digest.refresh')}
          >
            <RefreshCw size={15} aria-hidden />
          </button>
          <button
            className={styles.iconBtn}
            onClick={close}
            title={t('digest.close')}
            aria-label={t('digest.close')}
          >
            <X size={15} aria-hidden />
          </button>
        </header>

        {error ? <p className={styles.error}>{error}</p> : null}

        {loading && !latest ? (
          <p className={styles.empty}>{t('digest.loading')}</p>
        ) : latest ? (
          <div className={styles.body}>
            <p className={styles.meta}>
              {t('digest.meta', { when: fmt(latest.createdAt, i18n.language), count: latest.noteCount })}
            </p>
            <div className={styles.content}>{latest.content}</div>
          </div>
        ) : (
          <p className={styles.empty}>
            {generating ? t('digest.queued') : t('digest.empty')}
          </p>
        )}
      </div>
    </div>
  );
}
