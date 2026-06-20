import { RefreshCw } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { tauriApi } from '../../lib/tauri-api';
import styles from './InspectorRail.module.css';

/**
 * Inspector «Резюме» (макет editor.jsx): краткое LLM-резюме ТЕКУЩЕГО текста заметки (one-shot,
 * утилитарная модель через GuardedClient). Запрос — по открытию секции и при смене заметки (НЕ на каждый
 * keystroke: текст берём из ref на момент запроса); кнопка «Обновить» перегенерирует по актуальному
 * тексту. Пустой ответ / нет модели → честная заглушка. Egress только внутренний chat-провайдер.
 */
export function NoteSummary({ doc, path }: { doc: string; path: string }) {
  const { t } = useTranslation();
  const docRef = useRef(doc);
  docRef.current = doc;
  const [state, setState] = useState<'loading' | 'ready' | 'empty' | 'error'>('loading');
  const [summary, setSummary] = useState('');
  const reqRef = useRef(0);
  const mountedRef = useRef(true);

  // Размонтирование (закрытие секции/смена вкладки) → гасим setState запоздавшего ответа (как RelatedNotes).
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const run = useCallback(() => {
    const req = ++reqRef.current; // защита от гонки запросов (поздний ответ старого не перетрёт новый)
    setState('loading');
    tauriApi.suggest
      .noteSummary(docRef.current)
      .then((s) => {
        if (!mountedRef.current || req !== reqRef.current) return;
        if (s) {
          setSummary(s);
          setState('ready');
        } else {
          setState('empty');
        }
      })
      .catch(() => {
        if (mountedRef.current && req === reqRef.current) setState('error');
      });
  }, []);

  // По открытию секции и при смене заметки (path) — перегенерация по актуальному тексту.
  useEffect(() => {
    run();
  }, [path, run]);

  return (
    <div className={styles.summary}>
      <div className={styles.sumHead}>
        <span className={styles.sumLead}>{t('inspector.summaryLead')}</span>
        <button
          type="button"
          className={styles.sumRefresh}
          onClick={run}
          disabled={state === 'loading'}
          title={t('inspector.refresh')}
          aria-label={t('inspector.refresh')}
        >
          <RefreshCw size={13} aria-hidden />
        </button>
      </div>
      {state === 'loading' && <p className={styles.placeholder}>{t('inspector.loading')}</p>}
      {state === 'error' && <p className={styles.placeholder}>{t('inspector.error')}</p>}
      {state === 'empty' && <p className={styles.placeholder}>{t('inspector.summaryEmpty')}</p>}
      {state === 'ready' && (
        <p className={styles.sumBody} aria-live="polite">
          {summary}
        </p>
      )}
    </div>
  );
}
