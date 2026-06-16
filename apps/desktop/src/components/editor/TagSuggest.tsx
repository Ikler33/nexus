import { useState } from 'react';
import { Loader2, Tags } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { FlushFailedError } from '../../lib/frontmatter-edit';
import { applyTags, existingTags } from '../../lib/tag-apply';
import { tauriApi } from '../../lib/tauri-api';
import { useToastStore } from '../../stores/toast';
import styles from './TagSuggest.module.css';

/**
 * AI-2c: closed-vocab авто-тег. ПО КЛИКУ (LLM-вызов — не на каждом открытии заметки) `chat_util`
 * предлагает теги ТОЛЬКО из словаря vault; уже присутствующие в теле — отсеиваем (по `doc`). Выбранные
 * чипы «Применить» дописывает инлайн `#tag` в тело через безопасный `applyTags` (флаш→read→append→write).
 * Закрытость словаря гарантирует бэкенд (тег вне словаря в предложения не попадает).
 */
export function TagSuggest({ path, doc }: { path: string; doc: string }) {
  const { t } = useTranslation();
  const addToast = useToastStore((s) => s.addToast);
  const [phase, setPhase] = useState<'idle' | 'loading' | 'ready'>('idle');
  const [tags, setTags] = useState<string[]>([]); // предложенные НОВЫЕ теги (минус уже в теле)
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [applying, setApplying] = useState(false);

  const suggest = async () => {
    setPhase('loading');
    try {
      const res = await tauriApi.suggest.suggestTags(path);
      const present = existingTags(doc); // тело-инлайн ∪ frontmatter — не предлагаем уже-присутствующий
      const fresh = res.tags.filter((x) => !present.has(x.toLowerCase()));
      setTags(fresh);
      setSelected(new Set(fresh));
      setPhase('ready');
    } catch {
      setPhase('idle');
      addToast(t('tagSuggest.failed'), { kind: 'error' });
    }
  };

  const toggle = (tag: string) => {
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(tag)) n.delete(tag);
      else n.add(tag);
      return n;
    });
  };

  const reset = () => {
    setPhase('idle');
    setTags([]);
    setSelected(new Set());
  };

  const apply = async () => {
    const chosen = tags.filter((x) => selected.has(x));
    if (chosen.length === 0) return;
    setApplying(true);
    try {
      const added = await applyTags(path, chosen);
      addToast(
        added.length ? t('tagSuggest.applied', { count: added.length }) : t('tagSuggest.nothing'),
        { kind: 'success' },
      );
      reset();
    } catch (e) {
      addToast(
        e instanceof FlushFailedError ? t('tagSuggest.flushFailed') : t('tagSuggest.applyFailed'),
        { kind: 'error' },
      );
    } finally {
      setApplying(false);
    }
  };

  if (phase === 'idle') {
    return (
      <button type="button" className={styles.trigger} onClick={() => void suggest()}>
        <Tags size={13} aria-hidden /> {t('tagSuggest.suggest')}
      </button>
    );
  }
  if (phase === 'loading') {
    return (
      <div className={styles.bar} role="status">
        <Loader2 size={13} className={styles.spin} aria-hidden /> {t('tagSuggest.loading')}
      </div>
    );
  }
  if (tags.length === 0) {
    return (
      <div className={styles.bar}>
        <span className={styles.muted}>{t('tagSuggest.none')}</span>
        <button type="button" className={styles.link} onClick={reset}>
          {t('tagSuggest.dismiss')}
        </button>
      </div>
    );
  }
  return (
    <div className={styles.bar} role="group" aria-label={t('tagSuggest.suggest')}>
      <Tags size={13} aria-hidden />
      <div className={styles.chips}>
        {tags.map((tag) => (
          <button
            key={tag}
            type="button"
            className={selected.has(tag) ? styles.chipOn : styles.chip}
            aria-pressed={selected.has(tag)}
            onClick={() => toggle(tag)}
          >
            #{tag}
          </button>
        ))}
      </div>
      <button
        type="button"
        className={styles.apply}
        disabled={applying || selected.size === 0}
        onClick={() => void apply()}
      >
        {t('tagSuggest.apply')}
      </button>
      <button type="button" className={styles.link} onClick={reset}>
        {t('tagSuggest.dismiss')}
      </button>
    </div>
  );
}
