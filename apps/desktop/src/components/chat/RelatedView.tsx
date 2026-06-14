import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { useRelatedStore, visibleRelated } from '../../stores/related';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import { useRelationExplanations } from './useRelationExplanations';
import styles from './RelatedView.module.css';

/**
 * Тело вкладки «Похожие» (#35, дискавери): семантически близкие заметки для открытого файла, ВКЛЮЧАЯ
 * уже связанные. Порог релевантности — слайдер (настройка, D4); «вставить связь» дописывает `[[wikilink]]`
 * и НЕ убирает строку (AC-RN-6); клик по заголовку открывает заметку. Работает офлайн (max-sim из
 * сохранённых векторов).
 */
export function RelatedView() {
  const { t } = useTranslation();
  const path = useWorkspaceStore(activePath);
  const loading = useRelatedStore((s) => s.loading);
  const all = useRelatedStore((s) => s.items);
  const threshold = useRelatedStore((s) => s.threshold);
  const setThreshold = useRelatedStore((s) => s.setThreshold);
  const load = useRelatedStore((s) => s.load);
  const insertLink = useRelatedStore((s) => s.insertLink);
  const openFile = useWorkspaceStore((s) => s.openFile);
  // Объяснения только для ВИДИМЫХ по порогу карточек (не генерим LLM на скрытые) — AIP-10.
  const items = visibleRelated(all, threshold);
  const explanations = useRelationExplanations(path, items);

  useEffect(() => {
    void load(path);
  }, [path, load]);

  if (!path) {
    return <p className={styles.empty}>{t('related.noFile')}</p>;
  }

  return (
    <div className={styles.wrap}>
      <div className={styles.bar}>
        <span>{t('related.threshold')}</span>
        <input
          type="range"
          min={0}
          max={1}
          step={0.05}
          value={threshold}
          onChange={(e) => setThreshold(+e.target.value)}
          aria-label={t('related.threshold')}
        />
        <span className={styles.barVal}>{Math.round(threshold * 100)}%</span>
      </div>

      {loading ? (
        <p className={styles.empty}>{t('related.loading')}</p>
      ) : items.length === 0 ? (
        <p className={styles.empty}>{t('related.empty')}</p>
      ) : (
        <ul className={styles.list} aria-label={t('related.title')}>
          {items.map((s) => {
            const reason = explanations[s.path] || s.reason; // готово объяснение → оно; иначе сниппет
            return (
              <li key={s.path} className={styles.card}>
                <div className={styles.head}>
                  <button
                    type="button"
                    className={styles.title}
                    title={s.path}
                    onClick={() => void openFile(s.path)}
                  >
                    {s.title ?? s.path}
                  </button>
                  <span className={styles.score}>{Math.round(s.score * 100)}%</span>
                </div>
                {reason && <p className={styles.reason}>{reason}</p>}
                <button type="button" className={styles.insert} onClick={() => insertLink(s.path)}>
                  {t('related.insert')}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
