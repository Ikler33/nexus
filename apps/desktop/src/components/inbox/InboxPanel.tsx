import { useCallback, useEffect, useState } from 'react';
import { FilePlus, Inbox, ListChecks, RefreshCw, Trash2, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFocusTrap } from '../../hooks/useFocusTrap';
import { discard, loadInbox, toNote, toTask } from '../../lib/inbox/actions';
import type { InboxItem } from '../../lib/inbox/parse';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { BrandThinking } from '../chrome/BrandThinking';
import styles from './InboxPanel.module.css';

/**
 * Панель «Входящие» (INBOX-1, GTD-разбор): строки быстрого захвата из Inbox.md (CAP-2) с действиями
 * «В задачу» (→ дневник как `- [ ]`), «В заметку» (→ новая заметка), «Удалить». Каждое действие
 * вырезает строку из Inbox (буфер-aware, lib/inbox/actions) и перезагружает список (номера строк
 * сдвигаются). Превращает захват в обработанный поток. Офлайн, строгий CSP (текст как узлы).
 */
export function InboxPanel() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeInbox);
  const trapRef = useFocusTrap<HTMLDivElement>(close);
  const hasVault = useVaultStore((s) => s.info != null);
  const [items, setItems] = useState<InboxItem[]>([]);
  const [loading, setLoading] = useState(true);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setItems(await loadInbox());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (hasVault) void reload();
  }, [hasVault, reload]);

  /** Применяет действие; после успеха «В заметку» закрывает панель (открылась заметка), иначе —
   *  перезагружает список (строки сдвинулись / дрейф). */
  const run = async (
    item: InboxItem,
    action: (i: InboxItem) => Promise<boolean>,
    navigates = false,
  ) => {
    const ok = await action(item);
    if (ok && navigates) {
      close();
      return;
    }
    void reload();
  };

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        ref={trapRef}
        tabIndex={-1}
        className={styles.panel}
        role="dialog"
        aria-modal="true"
        aria-label={t('inbox.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <span className={styles.iconBox}>
            <Inbox size={16} aria-hidden />
          </span>
          <span className={styles.title}>{t('inbox.title')}</span>
          <span className={styles.spacer} />
          <button
            className={styles.iconBtn}
            onClick={() => void reload()}
            title={t('inbox.refresh')}
            aria-label={t('inbox.refresh')}
          >
            <RefreshCw size={15} aria-hidden />
          </button>
          <button
            className={styles.iconBtn}
            onClick={close}
            title={t('inbox.close')}
            aria-label={t('inbox.close')}
          >
            <X size={15} aria-hidden />
          </button>
        </header>

        {loading ? (
          <div className={styles.thinking}>
            <BrandThinking size={26} />
            <span className="mt-label">{t('inbox.loading')}</span>
          </div>
        ) : items.length === 0 ? (
          <div className={styles.emptyState}>
            <span className={styles.emptyIcoBox}>
              <Inbox size={22} className={styles.emptyIco} aria-hidden />
            </span>
            <p className={styles.empty}>{t('inbox.empty')}</p>
          </div>
        ) : (
          <ul className={styles.list}>
            {items.map((item) => (
              <li key={`${item.line}:${item.text}`} className={styles.row}>
                <span className={styles.time}>{item.time}</span>
                <span className={styles.text}>{item.text}</span>
                <button
                  className={styles.actBtn}
                  onClick={() => void run(item, toTask)}
                  title={t('inbox.toTask')}
                  aria-label={t('inbox.toTask')}
                >
                  <ListChecks size={14} aria-hidden />
                </button>
                <button
                  className={styles.actBtn}
                  onClick={() => void run(item, toNote, true)}
                  title={t('inbox.toNote')}
                  aria-label={t('inbox.toNote')}
                >
                  <FilePlus size={14} aria-hidden />
                </button>
                <button
                  className={`${styles.actBtn} ${styles.danger}`}
                  onClick={() => void run(item, discard)}
                  title={t('inbox.discard')}
                  aria-label={t('inbox.discard')}
                >
                  <Trash2 size={14} aria-hidden />
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
