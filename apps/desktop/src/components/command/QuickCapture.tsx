import { useState } from 'react';
import { Inbox } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { appendCapture } from '../../lib/daily';
import { useUIStore } from '../../stores/ui';
import styles from './QuickCapture.module.css';

/**
 * Quick-capture (CAP-2, ⌘⇧N): мини-модалка мгновенной записи мысли в `Inbox.md` БЕЗ открытия файла.
 * Enter — дозаписать строкой «- HH:MM …» и закрыть; Esc — отмена. Захват без трения (провал аудита).
 */
export function QuickCapture() {
  const open = useUIStore((s) => s.captureOpen);
  const close = useUIStore((s) => s.closeCapture);
  const { t } = useTranslation();
  const [text, setText] = useState('');

  if (!open) return null;

  const submit = () => {
    const v = text.trim();
    close();
    setText('');
    if (v) void appendCapture(v);
  };

  const cancel = () => {
    close();
    setText('');
  };

  return (
    <div className={styles.overlay} onClick={cancel} role="presentation">
      <div
        className={styles.box}
        role="dialog"
        aria-modal="true"
        aria-label={t('capture.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <Inbox size={16} aria-hidden className={styles.ico} />
        <input
          className={styles.input}
          autoFocus
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              submit();
            } else if (e.key === 'Escape') {
              e.preventDefault();
              cancel();
            }
          }}
          placeholder={t('capture.placeholder')}
          aria-label={t('capture.title')}
        />
        <kbd className={styles.kbd}>↵</kbd>
      </div>
    </div>
  );
}
