import { useEffect } from 'react';
import { RefreshCw, Scale, X } from 'lucide-react';
import { OrbitIcon } from '../common/BrandGlyphs';
import { useTranslation } from 'react-i18next';

import { useFocusTrap } from '../../hooks/useFocusTrap';
import { useAiFeaturesStore } from '../../stores/aiFeatures';
import { useContradictionsStore } from '../../stores/contradictions';
import { useUIStore } from '../../stores/ui';
import { useWorkspaceStore } from '../../stores/workspace';
import { BrandThinking } from '../common/BrandThinking';
import styles from './ContradictionsPanel.module.css';

/** Имя файла из пути (последний сегмент без .md). */
function noteName(path: string): string {
  const base = path.slice(path.lastIndexOf('/') + 1);
  return base.endsWith('.md') ? base.slice(0, -3) : base;
}

/**
 * Панель «Поиск противоречий» (#vision, спека `docs/specs/contradictions.md`): список найденных пар
 * конфликтующих/устаревших заметок (тип hard/soft/temporal + объяснение). Модалка из титлбара. Поиск
 * асинхронен (фоновая джоба) — результат прилетает по `jobs:changed` (refetch в App). Клик по заметке
 * открывает её.
 */
export function ContradictionsPanel() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeContradictions);
  const trapRef = useFocusTrap<HTMLDivElement>(close); // a11y: Esc/Tab-цикл внутри модалки (audit B10)
  const items = useContradictionsStore((s) => s.items);
  const loading = useContradictionsStore((s) => s.loading);
  const generating = useContradictionsStore((s) => s.generating);
  const error = useContradictionsStore((s) => s.error);
  const load = useContradictionsStore((s) => s.load);
  const generate = useContradictionsStore((s) => s.generate);
  // Тоггл «Поиск противоречий» (owner-gated, дефолт OFF): при OFF прячем кнопку запуска (была бы no-op)
  // и показываем честную подсказку «включите в настройках».
  const enabled = useAiFeaturesStore((s) => s.contradictions);
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
        aria-label={t('contradictions.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <span className={styles.iconBox}>
            <Scale size={16} aria-hidden />
          </span>
          <span className={styles.title}>{t('contradictions.title')}</span>
          <span className={styles.spacer} />
          {enabled && (
            <button
              className={styles.genBtn}
              onClick={() => void generate()}
              disabled={generating}
              title={t('contradictions.generate')}
            >
              <OrbitIcon size={14} aria-hidden />
              <span>
                {generating ? t('contradictions.generating') : t('contradictions.generate')}
              </span>
            </button>
          )}
          <button
            className={styles.iconBtn}
            onClick={() => void load()}
            title={t('contradictions.refresh')}
            aria-label={t('contradictions.refresh')}
          >
            <RefreshCw size={15} aria-hidden />
          </button>
          <button
            className={styles.iconBtn}
            onClick={close}
            title={t('contradictions.close')}
            aria-label={t('contradictions.close')}
          >
            <X size={15} aria-hidden />
          </button>
        </header>

        {error ? <p className={styles.error}>{error}</p> : null}

        {generating && items.length === 0 ? (
          // Поиск идёт: «думающий» бренд-знак с шиммером «Сверяю утверждения…» (макет).
          <div className={styles.thinking}>
            <BrandThinking size={30} />
            <span className="mt-label">{t('contradictions.thinking')}</span>
          </div>
        ) : loading && items.length === 0 ? (
          <p className={styles.empty}>{t('contradictions.loading')}</p>
        ) : items.length === 0 ? (
          <div className={styles.emptyState}>
            <span className={styles.emptyIcoBox}>
              <Scale size={22} className={styles.emptyIco} aria-hidden />
            </span>
            <p className={styles.empty}>
              {enabled ? t('contradictions.empty') : t('contradictions.disabled')}
            </p>
          </div>
        ) : (
          <ul className={styles.list}>
            {items.map((c, i) => (
              <li key={`${c.pathA}|${c.pathB}|${i}`} className={styles.row}>
                <div className={styles.pair}>
                  <button className={styles.note} title={c.pathA} onClick={() => open(c.pathA)}>
                    {noteName(c.pathA)}
                  </button>
                  <span className={styles.vs}>↔</span>
                  <button className={styles.note} title={c.pathB} onClick={() => open(c.pathB)}>
                    {noteName(c.pathB)}
                  </button>
                  <span className={`${styles.badge} ${styles[c.ctype] ?? ''}`}>
                    {t(`contradictions.type.${c.ctype}`, c.ctype)}
                  </span>
                </div>
                <p className={styles.explanation}>{c.explanation}</p>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
