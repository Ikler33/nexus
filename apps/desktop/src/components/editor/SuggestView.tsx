import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { useSuggestStore } from '../../stores/suggest';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import { useRelationExplanations } from './useRelationExplanations';
import styles from './SuggestView.module.css';

/**
 * Тело вкладки «Связи» (Ф1-9): карточки предложенных связей для открытого файла. Загрузка лениво
 * при показе вкладки и смене активного файла. Accept дописывает `[[wikilink]]`, dismiss прячет.
 */
export function SuggestView() {
  const { t } = useTranslation();
  const path = useWorkspaceStore(activePath);
  const items = useSuggestStore((s) => s.items);
  const loading = useSuggestStore((s) => s.loading);
  const load = useSuggestStore((s) => s.load);
  const accept = useSuggestStore((s) => s.accept);
  const dismiss = useSuggestStore((s) => s.dismiss);
  const explanations = useRelationExplanations(path, items); // AIP-10: LLM-причина связи vs сниппет

  useEffect(() => {
    void load(path);
  }, [path, load]);

  if (!path) {
    return <p className={styles.empty}>{t('suggest.noFile')}</p>;
  }
  if (loading) {
    return <p className={styles.empty}>{t('suggest.loading')}</p>;
  }
  if (items.length === 0) {
    return <p className={styles.empty}>{t('suggest.empty')}</p>;
  }

  return (
    <ul className={styles.list} aria-label={t('suggest.title')}>
      {items.map((s) => {
        const reason = explanations[s.path] || s.reason; // объяснение готово → оно; иначе сниппет
        return (
          <li key={s.path} className={styles.card}>
            <div className={styles.head}>
              <span className={styles.path} title={s.path}>
                {s.title ?? s.path}
              </span>
              <span className={styles.score}>{Math.round(s.score * 100)}%</span>
            </div>
            {reason && <p className={styles.reason}>{reason}</p>}
            <div className={styles.actions}>
              <button className={styles.accept} onClick={() => accept(s.path)}>
                {t('suggest.accept')}
              </button>
              <button className={styles.dismiss} onClick={() => dismiss(s.path)}>
                {t('suggest.dismiss')}
              </button>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
