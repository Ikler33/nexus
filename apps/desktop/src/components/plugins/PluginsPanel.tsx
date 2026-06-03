import { useEffect, useRef, useState } from 'react';
import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { type PluginCall, demoPluginSrcdoc, mountPlugin } from '../../lib/plugin-host';
import { useUIStore } from '../../stores/ui';
import styles from './PluginsPanel.module.css';

interface AuditRow extends PluginCall {
  id: number;
}

/**
 * Панель плагинов (Ф2): демонстрирует фронт-транспорт ADR-001/002 — демо-плагин крутится в
 * sandbox-iframe (`allow-scripts`, opaque origin) и зовёт host-функции через capability-брокер.
 * Лог справа показывает каждый брокерский вызов (метод/путь/исход) — и разрешённые, и отклонённые
 * по scope. Токен сессии живёт на host-стороне и плагину не передаётся.
 */
export function PluginsPanel() {
  const { t } = useTranslation();
  const closePlugins = useUIStore((s) => s.closePlugins);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const nextId = useRef(0);
  const [calls, setCalls] = useState<AuditRow[]>([]);

  useEffect(() => {
    const iframe = iframeRef.current;
    if (!iframe) return;
    let disposed = false;
    let handle: { dispose(): void } | undefined;

    void mountPlugin('hello', iframe, {
      onCall: (c) =>
        setCalls((prev) => [...prev, { id: nextId.current++, ...c }].slice(-50)),
    }).then((h) => {
      if (disposed) h.dispose();
      else handle = h;
    });

    return () => {
      disposed = true;
      handle?.dispose();
    };
  }, []);

  return (
    <div className={styles.backdrop} onClick={closePlugins} role="presentation">
      <div
        className={styles.dialog}
        role="dialog"
        aria-modal="true"
        aria-label={t('plugins.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          <span className={styles.title}>{t('plugins.title')}</span>
          <span className={styles.badge}>{t('plugins.sandbox')}</span>
          <button
            className={styles.close}
            onClick={closePlugins}
            aria-label={t('plugins.close')}
            title={t('plugins.close')}
          >
            <X size={16} aria-hidden />
          </button>
        </header>

        <div className={styles.body}>
          <iframe
            ref={iframeRef}
            className={styles.frame}
            title={t('plugins.title')}
            sandbox="allow-scripts"
            srcDoc={demoPluginSrcdoc()}
          />
          <aside className={styles.audit} aria-label={t('plugins.auditTitle')}>
            <h2 className={styles.auditHead}>{t('plugins.auditTitle')}</h2>
            {calls.length === 0 ? (
              <p className={styles.auditEmpty}>{t('plugins.auditEmpty')}</p>
            ) : (
              <ul className={styles.auditList}>
                {calls.map((c) => (
                  <li key={c.id} className={c.ok ? styles.ok : styles.denied}>
                    <span className={styles.verdict} aria-hidden>
                      {c.ok ? '✓' : '✋'}
                    </span>
                    <code className={styles.method}>{c.method}</code>
                    {c.path != null && <span className={styles.path}>{c.path || '/'}</span>}
                  </li>
                ))}
              </ul>
            )}
          </aside>
        </div>
      </div>
    </div>
  );
}
