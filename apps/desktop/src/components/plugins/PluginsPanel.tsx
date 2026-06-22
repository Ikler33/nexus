import { useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  ArrowLeft,
  Clock,
  Puzzle,
  Shield,
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import {
  type PluginCall,
  demoPluginSrcdoc,
  mountPlugin,
} from "../../lib/plugin-host";
import {
  tauriApi,
  type PermissionChip,
  type PluginInfo,
} from "../../lib/tauri-api";
import { useUIStore } from "../../stores/ui";
import styles from "./PluginsPanel.module.css";

interface AuditRow extends PluginCall {
  id: number;
}

/** Нав-вкладки менеджера (макет plugins.jsx): установленные + журнал доступа. */
type Nav = "installed" | "audit";

/** Персист consent-решений (DP-8): dir → разрешено. Отзыв — сброс записи. */
const CONSENT_KEY = "nexus.plugin.consent.v1";

function readConsent(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(CONSENT_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return typeof parsed === "object" && parsed !== null
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
  return p.permissions.some((c) => c.level !== "safe");
}

/**
 * Менеджер плагинов (QASR-views, макет `plugins.jsx`): левый нав-столбец (220px) с вкладками
 * «Установленные» / «Журнал доступа» + privacy-нота. Карточка плагина — 3-частная: glyph (44×44) +
 * тело (имя/версия/sandbox-бейдж/описание/perm-чипы по уровням риска) + side-действие «Запустить»
 * (песочница через consent-sheet для не-safe прав; решение персистится и отзывается). Запуск
 * монтирует demo-плагин в sandbox-iframe (Ф2); журнал брокер-вызовов — отдельная нав-вкладка.
 *
 * Бэкенд плагинов даёт только list/open/close/invoke — enable/disable/remove/marketplace отсутствуют,
 * поэтому соответствующих контролов в UI нет (это feature-work, не дизайн-слой).
 */
export function PluginsPanel() {
  const { t } = useTranslation();
  const closePlugins = useUIStore((s) => s.closePlugins);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const nextId = useRef(0);
  const [calls, setCalls] = useState<AuditRow[]>([]);
  const [nav, setNav] = useState<Nav>("installed");
  // Запущенный в песочнице плагин (его iframe смонтирован). null — список карточек.
  const [running, setRunning] = useState<PluginInfo | null>(null);
  const [plugins, setPlugins] = useState<PluginInfo[]>([]);
  const [consent, setConsent] = useState<Record<string, boolean>>(readConsent);
  const [sheet, setSheet] = useState<PluginInfo | null>(null);

  useEffect(() => {
    void tauriApi.plugins
      .list()
      .then(setPlugins)
      .catch(() => setPlugins([]));
  }, []);

  // Песочница монтируется только когда плагин запущен (и после consent'а).
  useEffect(() => {
    if (!running) return;
    const iframe = iframeRef.current;
    if (!iframe) return;
    let disposed = false;
    let handle: { dispose(): void } | undefined;

    void mountPlugin("hello", iframe, {
      onCall: (c) =>
        setCalls((prev) =>
          [...prev, { id: nextId.current++, ...c }].slice(-50),
        ),
    }).then((h) => {
      if (disposed) h.dispose();
      else handle = h;
    });

    return () => {
      disposed = true;
      handle?.dispose();
    };
  }, [running]);

  const launch = (p: PluginInfo) => {
    if (needsConsent(p) && !consent[p.dir]) {
      setSheet(p);
      return;
    }
    setRunning(p);
  };

  const allow = (p: PluginInfo) => {
    const next = { ...consent, [p.dir]: true };
    setConsent(next);
    persistConsent(next);
    setSheet(null);
    setRunning(p);
  };

  const revoke = (p: PluginInfo) => {
    const next = { ...consent };
    delete next[p.dir];
    setConsent(next);
    persistConsent(next);
  };

  const refresh = () =>
    void tauriApi.plugins
      .list()
      .then(setPlugins)
      .catch(() => setPlugins([]));

  // Включить/выключить плагин (персист на бэке) → обновить список. Выключение запущенного — гасим песочницу.
  const toggleEnabled = (p: PluginInfo) => {
    void tauriApi.plugins.setEnabled(p.dir, !p.enabled).then(() => {
      if (p.enabled && running?.dir === p.dir) setRunning(null);
      refresh();
    });
  };

  // Удалить плагин (в корзину .nexus/.trash, обратимо). Если запущен — закрыть песочницу.
  const removePlugin = (p: PluginInfo) => {
    void tauriApi.plugins.remove(p.dir).then(() => {
      if (running?.dir === p.dir) setRunning(null);
      refresh();
    });
  };

  // Двоеточие в kind конфликтует с nsSeparator i18next → ключи через подчёркивание.
  const permKey = (kind: string) => kind.replace(":", "_");

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
        aria-label={t("plugins.title")}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          <span className={styles.headIc} aria-hidden>
            <Puzzle size={19} />
          </span>
          <div className={styles.headTt}>
            <div className={styles.title}>{t("plugins.title")}</div>
            <div className={styles.subtitle}>{t("plugins.subtitle")}</div>
          </div>
          <button
            className={styles.close}
            onClick={closePlugins}
            aria-label={t("plugins.close")}
            title={t("plugins.close")}
          >
            <X size={16} aria-hidden />
          </button>
        </header>

        <nav className={styles.nav} aria-label={t("plugins.title")}>
          <button
            type="button"
            className={`${styles.navItem} ${nav === "installed" ? styles.navActive : ""}`}
            aria-current={nav === "installed"}
            onClick={() => setNav("installed")}
          >
            <Puzzle size={16} aria-hidden />
            {t("plugins.installed")}
            <span className={styles.cnt}>{plugins.length}</span>
          </button>
          <button
            type="button"
            className={`${styles.navItem} ${nav === "audit" ? styles.navActive : ""}`}
            aria-current={nav === "audit"}
            onClick={() => setNav("audit")}
          >
            <Clock size={16} aria-hidden />
            {t("plugins.auditTab")}
          </button>
          <div className={styles.navNote}>
            <Shield size={15} className={styles.navNoteIc} aria-hidden />
            {t("plugins.privacyNote")}
          </div>
        </nav>

        <main className={styles.main}>
          {nav === "installed" &&
            (running ? (
              <div className={styles.sandbox}>
                <div className={styles.sandboxBar}>
                  <button
                    type="button"
                    className={styles.back}
                    onClick={() => setRunning(null)}
                  >
                    <ArrowLeft size={14} aria-hidden />
                    {t("plugins.back")}
                  </button>
                  <span className={styles.sandboxName}>
                    {running.name ?? running.dir}
                  </span>
                  <span
                    className={styles.sandboxTag}
                    title={t("plugins.sandbox")}
                  >
                    <Shield size={10} aria-hidden />
                    {t("plugins.sandbox")}
                  </span>
                </div>
                <iframe
                  ref={iframeRef}
                  className={styles.frame}
                  title={running.name ?? running.dir}
                  sandbox="allow-scripts"
                  srcDoc={demoPluginSrcdoc()}
                />
              </div>
            ) : (
              <div className={styles.cards}>
                {plugins.length === 0 && (
                  <p className={styles.auditEmpty}>{t("plugins.empty")}</p>
                )}
                {plugins.map((p) => (
                  <div
                    key={p.dir}
                    className={`${styles.card} ${p.enabled ? "" : styles.cardOff}`}
                  >
                    <span
                      className={`${styles.glyph} ${p.enabled ? styles.glyphOn : ""}`}
                      aria-hidden
                    >
                      <Puzzle size={22} />
                    </span>
                    <div className={styles.cardBody}>
                      <div className={styles.nameLine}>
                        <strong>{p.name ?? p.dir}</strong>
                        {p.version && (
                          <span className={styles.ver}>v{p.version}</span>
                        )}
                        <span
                          className={styles.sandboxBadge}
                          title={t("plugins.sandbox")}
                        >
                          <Shield size={10} aria-hidden />
                          {t("plugins.sandbox")}
                        </span>
                        {!p.compatible && (
                          <span className={styles.incompat}>
                            <AlertTriangle size={11} aria-hidden />
                            {t("plugins.incompatible")}
                          </span>
                        )}
                      </div>
                      {p.error && (
                        <div className={styles.cardErr}>{p.error}</div>
                      )}
                      <div className={styles.perms}>
                        {p.permissions.map(chip)}
                      </div>
                      {consent[p.dir] && (
                        <div className={styles.consentLine}>
                          <ShieldCheck size={12} aria-hidden />
                          {t("plugins.consentGiven")}
                          <button
                            type="button"
                            className={styles.revoke}
                            onClick={() => revoke(p)}
                          >
                            {t("plugins.revoke")}
                          </button>
                        </div>
                      )}
                    </div>
                    <div className={styles.side}>
                      <label
                        className={styles.toggle}
                        title={
                          p.enabled ? t("plugins.disable") : t("plugins.enable")
                        }
                      >
                        <input
                          type="checkbox"
                          role="switch"
                          checked={p.enabled}
                          onChange={() => toggleEnabled(p)}
                          aria-label={
                            p.enabled
                              ? t("plugins.disable")
                              : t("plugins.enable")
                          }
                        />
                        <span className={styles.toggleTrack} aria-hidden />
                      </label>
                      <button
                        type="button"
                        className={styles.launch}
                        disabled={!p.compatible || !p.enabled}
                        onClick={() => launch(p)}
                      >
                        {t("plugins.launch")}
                      </button>
                      <button
                        type="button"
                        className={styles.remove}
                        onClick={() => removePlugin(p)}
                        title={t("plugins.remove")}
                        aria-label={t("plugins.remove")}
                      >
                        <Trash2 size={14} aria-hidden />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            ))}

          {nav === "audit" && (
            <div className={styles.audit} aria-label={t("plugins.auditTitle")}>
              <p className={styles.auditSub}>{t("plugins.auditSub")}</p>
              {calls.length === 0 ? (
                <p className={styles.auditEmpty}>{t("plugins.auditEmpty")}</p>
              ) : (
                <ul className={styles.auditList}>
                  {calls.map((c) => (
                    <li key={c.id} className={c.ok ? styles.ok : styles.denied}>
                      <span className={styles.verdict} aria-hidden>
                        {c.ok ? "✓" : "✋"}
                      </span>
                      <code className={styles.method}>{c.method}</code>
                      {c.path != null && (
                        <span className={styles.path}>{c.path || "/"}</span>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </main>

        {sheet && (
          <div
            className={styles.consentScrim}
            role="presentation"
            onClick={() => setSheet(null)}
          >
            <div
              className={styles.consent}
              role="dialog"
              aria-modal="true"
              aria-label={t("plugins.consentTitle", {
                name: sheet.name ?? sheet.dir,
              })}
              onClick={(e) => e.stopPropagation()}
            >
              <div className={styles.consentTop}>
                <span className={styles.consentGlyph} aria-hidden>
                  <Puzzle size={26} />
                </span>
                <div className={styles.consentName}>
                  {sheet.name ?? sheet.dir}
                </div>
                <div className={styles.consentSub}>
                  {t("plugins.consentSub")}
                </div>
              </div>
              <div className={styles.permRows}>
                {sheet.permissions.map((c) => (
                  <div
                    key={c.kind}
                    className={`${styles.permRow} ${styles[c.level]}`}
                  >
                    <span className={styles.prBadge} aria-hidden>
                      {c.level === "sensitive"
                        ? "!"
                        : c.level === "caution"
                          ? "~"
                          : "✓"}
                    </span>
                    <span className={styles.prText}>
                      <span className={styles.prTitle}>
                        {t(`plugins.perm.${permKey(c.kind)}.title`, {
                          defaultValue: c.kind,
                        })}
                      </span>
                      <span className={styles.prDesc}>
                        {t(`plugins.perm.${permKey(c.kind)}.desc`, {
                          defaultValue: "",
                        })}
                        {c.detail ? ` · ${c.detail}` : ""}
                      </span>
                    </span>
                  </div>
                ))}
              </div>
              <div className={styles.consentFoot}>
                <button
                  type="button"
                  className={styles.cancel}
                  onClick={() => setSheet(null)}
                >
                  {t("plugins.cancel")}
                </button>
                <button
                  type="button"
                  className={styles.allow}
                  onClick={() => allow(sheet)}
                >
                  <ShieldCheck size={15} aria-hidden />
                  {t("plugins.allow")}
                </button>
              </div>
              <div className={styles.consentNote}>
                <ShieldCheck size={12} aria-hidden />
                {t("plugins.revocableNote")}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
