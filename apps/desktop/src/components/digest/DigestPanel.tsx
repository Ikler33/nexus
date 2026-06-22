import { useEffect } from 'react';
import { Newspaper, RefreshCw, X } from 'lucide-react';
import { OrbitIcon } from '../chrome/BrandGlyphs';
import { useTranslation } from 'react-i18next';

import { useFocusTrap } from '../../hooks/useFocusTrap';
import { renderBold } from '../../lib/render';
import { useDigestStore } from '../../stores/digest';
import { useUIStore } from '../../stores/ui';
import { BrandThinking } from '../chrome/BrandThinking';
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
  const trapRef = useFocusTrap<HTMLDivElement>(close); // a11y: Esc/Tab-цикл внутри модалки (audit B10)
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
        ref={trapRef}
        tabIndex={-1}
        className={styles.panel}
        role="dialog"
        aria-modal="true"
        aria-label={t('digest.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <span className={styles.iconBox}>
            <Newspaper size={16} aria-hidden />
          </span>
          <span className={styles.title}>{t('digest.title')}</span>
          <span className={styles.spacer} />
          <button
            className={styles.genBtn}
            onClick={() => void generate()}
            disabled={generating}
            title={t('digest.generate')}
          >
            <OrbitIcon size={14} aria-hidden />
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

        {generating && !latest ? (
          // Генерация идёт: «думающий» бренд-знак с шиммером (макет insights.jsx).
          <div className={styles.thinking}>
            <BrandThinking size={30} />
            <span className="mt-label">{t('digest.thinking')}</span>
          </div>
        ) : loading && !latest ? (
          <p className={styles.empty}>{t('digest.loading')}</p>
        ) : latest ? (
          <div className={styles.body}>
            <p className={styles.meta}>
              {t('digest.meta', { when: fmt(latest.createdAt, i18n.language), count: latest.noteCount })}
              <span className={styles.aiBadge}>AI</span>
            </p>
            <div className={styles.content}>{renderBold(latest.content)}</div>
          </div>
        ) : (
          <div className={styles.emptyState}>
            <span className={styles.emptyIcoBox}>
              <Newspaper size={22} className={styles.emptyIco} aria-hidden />
            </span>
            <p className={styles.empty}>{t('digest.empty')}</p>
          </div>
        )}
      </div>
    </div>
  );
}
