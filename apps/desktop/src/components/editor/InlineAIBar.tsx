import { Check, RotateCcw, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { tauriApi } from '../../lib/tauri-api';
import { BrandThinking } from '../common/BrandThinking';
import styles from './InlineAIBar.module.css';

type Phase = 'ask' | 'thinking' | 'streaming' | 'done';

/**
 * InlineAI prompt-box (⌘/ или `/ai`, дизайн Qasr `editor.jsx` InlineAI): свободный запрос к LLM,
 * заземлённый на текущую заметку (`note`), со стримом и фазами ask→thinking→streaming→done. На done —
 * Вставить (вызывает `onInsert` — родитель вставляет в позицию курсора) / Заново / Отмена. Ортогонален
 * ghost-тексту (`inlineGhost.ts`): другой триггер/UX. Один активный стрим; отмена на размонтировании.
 */
export function InlineAIBar({
  note,
  onInsert,
  onClose,
}: {
  note: string;
  onInsert: (text: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [q, setQ] = useState('');
  const [phase, setPhase] = useState<Phase>('ask');
  const [out, setOut] = useState('');
  const [err, setErr] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const insertRef = useRef<HTMLButtonElement>(null);
  const cancelRef = useRef<(() => void) | null>(null);
  const accRef = useRef('');
  const mountedRef = useRef(true);

  // Фокус в инпут при открытии; гасим активный стрим при размонтировании (смена вкладки/пана, закрытие)
  // + ставим флаг, чтобы запоздавший токен (Tauri-канал отменяется асинхронно) не дёргал setState.
  useEffect(() => {
    inputRef.current?.focus();
    return () => {
      mountedRef.current = false;
      cancelRef.current?.();
    };
  }, []);

  // На done переводим фокус на «Вставить» (Enter сразу вставляет).
  useEffect(() => {
    if (phase === 'done') insertRef.current?.focus();
  }, [phase]);

  function run() {
    const query = q.trim();
    if (!query) return;
    cancelRef.current?.();
    accRef.current = '';
    setOut('');
    setErr(null);
    setPhase('thinking');
    cancelRef.current = tauriApi.inline.complete(
      'prompt',
      note,
      undefined,
      (ev) => {
        if (!mountedRef.current) return; // запоздавший токен после размонтирования — игнор
        switch (ev.type) {
          case 'token':
            accRef.current += ev.text;
            setOut(accRef.current);
            setPhase('streaming');
            break;
          case 'done': {
            const full = (ev.full || accRef.current).trim();
            cancelRef.current = null;
            if (!full) {
              setErr(t('inline.ai.empty'));
              setPhase('ask');
              return;
            }
            accRef.current = full;
            setOut(full);
            setPhase('done');
            break;
          }
          case 'error':
            cancelRef.current = null;
            setErr(ev.message);
            setPhase('ask');
            break;
        }
      },
      query,
    );
  }

  function retry() {
    cancelRef.current?.();
    cancelRef.current = null;
    accRef.current = '';
    setOut('');
    setErr(null);
    setPhase('ask');
    inputRef.current?.focus();
  }

  return (
    // role="dialog" + stopPropagation mousedown: клик в бар не снимает выделение/фокус редактора.
    <div
      className={styles.inlineAi}
      role="dialog"
      aria-label={t('inline.ai.label')}
      onMouseDown={(e) => e.stopPropagation()}
    >
      <div className={styles.iaBar}>
        <span className={styles.iaGlyph} aria-hidden>
          <BrandThinking size={16} />
        </span>
        {phase === 'ask' ? (
          <input
            ref={inputRef}
            className={styles.iaInput}
            value={q}
            placeholder={t('inline.ai.placeholder')}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                run();
              } else if (e.key === 'Escape') {
                e.preventDefault();
                onClose();
              }
            }}
          />
        ) : (
          <span className={styles.iaQ}>{q}</span>
        )}
        <button className={styles.closeBtn} onClick={onClose} title="Esc" aria-label={t('inline.ai.cancel')}>
          <X size={14} aria-hidden />
        </button>
      </div>

      {err && (
        <div className={styles.iaErr} role="alert">
          {err}
        </div>
      )}

      {phase === 'thinking' && (
        <div className={styles.iaStatus} role="status">
          {t('inline.ai.thinking')}
        </div>
      )}

      {(phase === 'streaming' || phase === 'done') && (
        <div className={styles.iaOut} aria-live="polite">
          {out}
          {phase === 'streaming' && <span className={styles.iaCaret} aria-hidden />}
        </div>
      )}

      {phase === 'done' && (
        <div className={styles.iaActions}>
          <button ref={insertRef} className={styles.primary} onClick={() => onInsert(out)}>
            <Check size={13} aria-hidden /> {t('inline.ai.insert')}
          </button>
          <button className={styles.iaAct} onClick={retry}>
            <RotateCcw size={13} aria-hidden /> {t('inline.ai.retry')}
          </button>
          <button className={styles.iaAct} onClick={onClose}>
            {t('inline.ai.cancel')}
          </button>
        </div>
      )}
    </div>
  );
}
