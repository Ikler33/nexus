import { useEffect, useRef, useState } from 'react';
import { AlertTriangle, Puzzle, ShieldCheck, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { type PluginCall, demoPluginSrcdoc, mountPlugin } from '../../lib/plugin-host';
import { tauriApi, type PermissionChip, type PluginInfo } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import styles from './PluginsPanel.module.css';

interface AuditRow extends PluginCall {
  id: number;
}

type Tab = 'installed' | 'sandbox';

/** Персист consent-решений (DP-8): dir → разрешено. Отзыв — сброс записи. */
const CONSENT_KEY = 'nexus.plugin.consent.v1';

function readConsent(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(CONSENT_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return typeof parsed === 'object' && parsed !== null
      ? (parsed as Record<string, boolean>)
      : {};
  } catch {
    return {};
  }
}

function persistConsent(map: Record<string, boolean>): void {
  try {
    localStorage.setItem(CONSENT_KEY, JSON.stringify(map));
  } catch {
    /* ignore */
  }
}

/** Не-safe права требуют информированного согласия перед запуском (макет plugins.jsx). */
function needsConsent(p: PluginInfo): boolean {
  return p.permissions.some((c) => c.level !== 'safe');
}

/**
 * Менеджер плагинов (DP-8, макет `plugins.jsx`): вкладка «Установленные» — карточки с
 * **чипами прав по уровням риска** (safe/caution/sensitive) и запуском в песочнице через
 * **consent-sheet** (для не-safe прав; решение персистится и отзывается); вкладка
 * «Песочница» — demo-плагин в sandbox-iframe + журнал брокер-вызовов (Ф2).
 */
export function PluginsPanel() {
  const { t } = useTranslation();
  const closePlugins = useUIStore((s) => s.closePlugins);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const nextId = useRef(0);
  const [calls, setCalls] = useState<AuditRow[]>([]);
  const [tab, setTab] = useState<Tab>('installed');
  const [plugins, setPlugins] = useState<PluginInfo[]>([]);
  const [consent, setConsent] = useState<Record<string, boolean>>(readConsent);
  const [sheet, setSheet] = useState<PluginInfo | null>(null);

  useEffect(() => {
    void tauriApi.plugins
      .list()
      .then(setPlugins)
      .catch(() => setPlugins([]));
  }, []);

  // Песочница монтируется только на своей вкладке (и после consent'а).
  useEffect(() => {
    if (tab !== 'sandbox') return;
    const iframe = iframeRef.current;
    if (!iframe) return;
    let disposed = false;
    let handle: { dispose(): void } | undefined;

    void mountPlugin('hello', iframe, {
      onCall: (c) => setCalls((prev) => [...prev, { id: nextId.current++, ...c }].slice(-50)),
    }).then((h) => {
      if (disposed) h.dispose();
      else handle = h;
    });

    return () => {
      disposed = true;
      handle?.dispose();
    };
  }, [tab]);

  const launch = (p: PluginInfo) => {
    if (needsConsent(p) && !consent[p.dir]) {
      setSheet(p);
      return;
    }
    setTab('sandbox');
  };

  const allow = (p: PluginInfo) => {
    const next = { ...consent, [p.dir]: true };
    setConsent(next);
    persistConsent(next);
    setSheet(null);
    setTab('sandbox');
  };

  const revoke = (p: PluginInfo) => {
    const next = { ...consent };
    delete next[p.dir];
    setConsent(next);
    persistConsent(next);
  };

  // Двоеточие в kind конфликтует с nsSeparator i18next → ключи через подчёркивание.
  const permKey = (kind: string) => kind.replace(':', '_');

  const chip = (c: PermissionChip) => (
    <span
      key={c.kind}
      className={`${styles.permChip} ${styles[c.level]}`}
      title={c.detail || undefined}
    >
      {t(`plugins.perm.${permKey(c.kind)}.title`, { defaultValue: c.kind })}
    </span>
  );

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
          <Puzzle size={17} aria-hidden />
          <span className={styles.title}>{t('plugins.title')}</span>
          <span className={styles.badge}>{t('plugins.sandbox')}</span>
          <div className={styles.tabs} role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'installed'}
              className={`${styles.tabBtn} ${tab === 'installed' ? styles.tabOn : ''}`}
              onClick={() => setTab('installed')}
            >
              {t('plugins.installed')}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'sandbox'}
              className={`${styles.tabBtn} ${tab === 'sandbox' ? styles.tabOn : ''}`}
              onClick={() => setTab('sandbox')}
            >
              {t('plugins.sandboxTab')}
            </button>
          </div>
          <button
            className={styles.close}
            onClick={closePlugins}
            aria-label={t('plugins.close')}
            title={t('plugins.close')}
          >
            <X size={16} aria-hidden />
          </button>
        </header>

        {tab === 'installed' && (
          <div className={styles.cards}>
            <p className={styles.privacyNote}>{t('plugins.privacyNote')}</p>
            {plugins.length === 0 && <p className={styles.auditEmpty}>{t('plugins.empty')}</p>}
            {plugins.map((p) => (
              <div key={p.dir} className={styles.card}>
                <Puzzle size={20} className={styles.glyph} aria-hidden />
                <div className={styles.cardBody}>
                  <div className={styles.nameLine}>
                    <strong>{p.name ?? p.dir}</strong>
                    {p.version && <span className={styles.ver}>v{p.version}</span>}
                    {!p.compatible && (
                      <span className={styles.incompat}>
                        <AlertTriangle size={11} aria-hidden />
                        {t('plugins.incompatible')}
                      </span>
                    )}
                  </div>
                  {p.error && <div className={styles.cardErr}>{p.error}</div>}
                  <div className={styles.perms}>{p.permissions.map(chip)}</div>
                  {consent[p.dir] && (
                    <div className={styles.consentLine}>
                      <ShieldCheck size={12} aria-hidden />
                      {t('plugins.consentGiven')}
                      <button
                        type="button"
                        className={styles.revoke}
                        onClick={() => revoke(p)}
                      >
                        {t('plugins.revoke')}
                      </button>
                    </div>
                  )}
                </div>
                <button
                  type="button"
                  className={styles.launch}
                  disabled={!p.compatible}
                  onClick={() => launch(p)}
                >
                  {t('plugins.launch')}
                </button>
              </div>
            ))}
          </div>
        )}

        {tab === 'sandbox' && (
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
        )}

        {sheet && (
          <div className={styles.consentScrim} role="presentation" onClick={() => setSheet(null)}>
            <div
              className={styles.consent}
              role="dialog"
              aria-modal="true"
              aria-label={t('plugins.consentTitle', { name: sheet.name ?? sheet.dir })}
              onClick={(e) => e.stopPropagation()}
            >
              <div className={styles.consentTop}>
                <Puzzle size={26} className={styles.glyph} aria-hidden />
                <div>
                  <div className={styles.consentName}>{sheet.name ?? sheet.dir}</div>
                  <div className={styles.consentSub}>{t('plugins.consentSub')}</div>
                </div>
              </div>
              <div className={styles.permRows}>
                {sheet.permissions.map((c) => (
                  <div key={c.kind} className={`${styles.permRow} ${styles[c.level]}`}>
                    <span className={styles.prBadge} aria-hidden>
                      {c.level === 'sensitive' ? '!' : c.level === 'caution' ? '~' : '✓'}
                    </span>
                    <span className={styles.prText}>
                      <span className={styles.prTitle}>
                        {t(`plugins.perm.${permKey(c.kind)}.title`, { defaultValue: c.kind })}
                      </span>
                      <span className={styles.prDesc}>
                        {t(`plugins.perm.${permKey(c.kind)}.desc`, { defaultValue: '' })}
                        {c.detail ? ` · ${c.detail}` : ''}
                      </span>
                    </span>
                  </div>
                ))}
              </div>
              <div className={styles.consentFoot}>
                <button type="button" className={styles.cancel} onClick={() => setSheet(null)}>
                  {t('plugins.cancel')}
                </button>
                <button type="button" className={styles.allow} onClick={() => allow(sheet)}>
                  <ShieldCheck size={15} aria-hidden />
                  {t('plugins.allow')}
                </button>
              </div>
              <div className={styles.consentNote}>
                <ShieldCheck size={12} aria-hidden />
                {t('plugins.revocableNote')}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
