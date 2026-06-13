import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useToastStore } from '../../stores/toast';
import styles from './ToastViewport.module.css';

/**
 * Стек тостов (TOAST-1, решение плана #15): bottom-left, новые снизу. Ошибки — `role="alert"`
 * (ассертивно зачитываются скринридером), остальные — `role="status"` (вежливо). Авто-исчезновение
 * и FIFO-кап — в сторе; здесь только рендер + ручное закрытие/действие.
 */
export function ToastViewport() {
  const toasts = useToastStore((s) => s.toasts);
  const dismiss = useToastStore((s) => s.dismiss);
  const { t } = useTranslation();

  if (toasts.length === 0) return null;

  return (
    <div className={styles.viewport} aria-live="polite" aria-relevant="additions">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={`${styles.toast} ${styles[toast.kind]}`}
          role={toast.kind === 'error' ? 'alert' : 'status'}
        >
          <span className={styles.msg}>{toast.message}</span>
          {toast.action && (
            <button
              type="button"
              className={styles.action}
              onClick={() => {
                toast.action?.run();
                dismiss(toast.id);
              }}
            >
              {toast.action.label}
            </button>
          )}
          <button
            type="button"
            className={styles.close}
            onClick={() => dismiss(toast.id)}
            aria-label={t('toast.dismiss')}
          >
            <X size={13} aria-hidden />
          </button>
        </div>
      ))}
    </div>
  );
}
