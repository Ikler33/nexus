import { useCallback, useEffect, useRef, useState } from 'react';
import {
  Check,
  FilePlus2,
  HardDrive,
  History,
  RefreshCw,
  Sparkles,
  SquarePen,
  WifiOff,
  X,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { logUi } from '../../lib/debug-log';
import { tauriApi, type ChatSessionInfo } from '../../lib/tauri-api';
import { usePrefsStore } from '../../stores/prefs';
import { useChatStore } from '../../stores/chat';
import { useRelatedStore } from '../../stores/related';
import { useSuggestStore } from '../../stores/suggest';
import { useUIStore } from '../../stores/ui';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import { ChatView } from './ChatView';
import { RelatedView } from './RelatedView';
import { SuggestView } from './SuggestView';
import styles from './AiPanel.module.css';

/**
 * Бейдж провайдера (E9, макет `.provider`): «Локально» (все модели — свои хосты) или «Офлайн»
 * (kill-switch egress). «Облако» появится со срезом 3 (cloud-fallback) — вариант зарезервирован.
 * Состояние читается при маунте панели (меняется редко — в настройках).
 */
function ProviderBadge() {
  const { t } = useTranslation();
  const [offline, setOffline] = useState(false);
  useEffect(() => {
    let cancelled = false;
    tauriApi.egress
      .getState()
      .then((s) => {
        if (!cancelled) setOffline(s.offline);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);
  return (
    <span
      className={`${styles.provider} ${offline ? styles.providerOffline : ''}`}
      title={t('chat.localHint')}
    >
      {offline ? <WifiOff size={12} aria-hidden /> : <HardDrive size={12} aria-hidden />}
      {offline ? t('chat.providerOffline') : t('chat.providerLocal')}
    </span>
  );
}

/** Группа истории по свежести (Claude-style: Сегодня / Вчера / Неделя / Ранее). */
function bucketOf(updatedAt: number, now: number): 'today' | 'yesterday' | 'week' | 'earlier' {
  const days = Math.floor((now - updatedAt) / 86_400);
  if (days <= 0) return 'today';
  if (days === 1) return 'yesterday';
  if (days < 7) return 'week';
  return 'earlier';
}

/**
 * История сессий (решение владельца 2026-06-12, вариант А «как в Claude/ChatGPT»): кнопка-часы
 * в шапке панели → glass-дропдаун с группировкой по датам; клик — загрузить сессию; на ховере
 * строки — «Сохранить в заметки» (экспорт в `Chats/…md`). Ничего не удаляем — это память
 * «второго мозга».
 */
function SessionHistory() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [sessions, setSessions] = useState<ChatSessionInfo[]>([]);
  const [savedId, setSavedId] = useState<number | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const sessionId = useChatStore((s) => s.sessionId);
  const loadSession = useChatStore((s) => s.loadSession);

  useEffect(() => {
    if (!open) return;
    void tauriApi.chat.sessions
      .list()
      .then(setSessions)
      .catch(() => setSessions([]));
    const onDown = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener('mousedown', onDown);
    return () => window.removeEventListener('mousedown', onDown);
  }, [open]);

  const toNote = (id: number) => {
    logUi('chat:session-to-note', String(id));
    void tauriApi.chat.sessions
      .toNote(id)
      .then(() => {
        setSavedId(id);
        setTimeout(() => setSavedId(null), 1800);
      })
      .catch(() => {});
  };

  const now = Math.floor(Date.now() / 1000);
  const buckets = (['today', 'yesterday', 'week', 'earlier'] as const)
    .map((b) => [b, sessions.filter((s) => bucketOf(s.updatedAt, now) === b)] as const)
    .filter(([, list]) => list.length > 0);

  return (
    <div className={styles.histWrap} ref={wrapRef}>
      <button
        className={`${styles.iconBtn} ${open ? styles.iconBtnOn : ''}`}
        onClick={() => setOpen((v) => !v)}
        title={t('chat.history')}
        aria-label={t('chat.history')}
        aria-expanded={open}
        aria-haspopup="menu"
      >
        <History size={15} aria-hidden />
      </button>
      {open && (
        <div className={styles.histMenu} role="menu" aria-label={t('chat.history')}>
          <div className={styles.histHead}>{t('chat.history')}</div>
          {buckets.length === 0 && <div className={styles.histEmpty}>{t('chat.historyEmpty')}</div>}
          {buckets.map(([bucket, list]) => (
            <div key={bucket}>
              <div className={styles.histBucket}>{t(`chat.hist.${bucket}`)}</div>
              {list.map((sess, i) => (
                <div
                  key={sess.id}
                  className={`${styles.histRow} ${sess.id === sessionId ? styles.histActive : ''}`}
                  style={{ animationDelay: `${Math.min(i, 8) * 22}ms` }}
                >
                  <button
                    type="button"
                    className={styles.histTitle}
                    role="menuitem"
                    onClick={() => {
                      setOpen(false);
                      logUi('chat:load-session', String(sess.id));
                      void loadSession(sess.id);
                    }}
                  >
                    {sess.title}
                  </button>
                  <button
                    type="button"
                    className={styles.histNote}
                    onClick={() => toNote(sess.id)}
                    title={t('chat.toNote')}
                    aria-label={t('chat.toNote')}
                  >
                    {savedId === sess.id ? (
                      <Check size={13} aria-hidden />
                    ) : (
                      <FilePlus2 size={13} aria-hidden />
                    )}
                  </button>
                </div>
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * AI-панель по макету `ai-panel.jsx` (DP-12): шапка ai-head (глиф + «AI-ассистент» + бейдж
 * провайдера + действия), табы отдельной строкой с подчёркиванием активного. Вкладки: «Чат»
 * (RAG, Ф1-8), «Связи» (предложения, Ф1-9), «Похожие» (#35); Summary-таб макета не переносим —
 * суммаризация живёт в inline-LLM редактора (honest-адаптация, BACKLOG).
 */
export function AiPanel({ variant = 'side' }: { variant?: 'side' | 'bottom' | 'overlay' }) {
  const { t } = useTranslation();
  const tab = useUIStore((s) => s.aiTab);
  const setTab = useUIStore((s) => s.setAiTab);
  const closeChat = useUIStore((s) => s.closeChat);
  const panelClass =
    variant === 'overlay'
      ? styles.panelOverlay
      : variant === 'bottom'
        ? `${styles.panel} ${styles.panelBottom}`
        : styles.panel;

  const messages = useChatStore((s) => s.messages);
  const streaming = useChatStore((s) => s.streaming);
  const newSession = useChatStore((s) => s.newSession);

  const reloadSuggest = useSuggestStore((s) => s.load);
  const reloadRelated = useRelatedStore((s) => s.load);
  const path = useWorkspaceStore(activePath);

  // Драг-ресайз панели (фидбэк владельца 11.06): тянем левую кромку (side) / верхнюю (bottom);
  // размер живёт в prefs (персист) и применяется грид-переменной App. Overlay не ресайзим.
  const setW = usePrefsStore((s) => s.setAiPanelW);
  const setH = usePrefsStore((s) => s.setAiPanelH);
  const startResize = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      const horizontal = variant === 'side';
      const onMove = (ev: MouseEvent) => {
        if (horizontal) setW(window.innerWidth - ev.clientX);
        else setH(window.innerHeight - ev.clientY);
      };
      const onUp = () => {
        window.removeEventListener('mousemove', onMove);
        window.removeEventListener('mouseup', onUp);
        document.body.style.cursor = '';
      };
      document.body.style.cursor = horizontal ? 'col-resize' : 'row-resize';
      window.addEventListener('mousemove', onMove);
      window.addEventListener('mouseup', onUp);
    },
    [variant, setW, setH],
  );

  return (
    <aside className={panelClass} aria-label={t('chat.title2')}>
      {variant !== 'overlay' && (
        <div
          className={`${styles.resizer} ${variant === 'bottom' ? styles.resizerH : styles.resizerV}`}
          onMouseDown={startResize}
          role="separator"
          aria-orientation={variant === 'bottom' ? 'horizontal' : 'vertical'}
          aria-label={t('chat.resize')}
          title={t('chat.resize')}
        />
      )}
      <header className={styles.head}>
        <span className={styles.headTitle}>
          <Sparkles size={16} aria-hidden />
          {t('chat.title2')}
        </span>
        <span className={styles.headSpacer} />
        <ProviderBadge />
        {tab === 'chat' ? (
          // Решение владельца 2026-06-12: ничего не удаляем — вместо корзины «История» и
          // «Новая сессия» (текущая лента уходит в память «второго мозга», не стирается).
          <>
            <SessionHistory />
            <button
              className={styles.iconBtn}
              onClick={() => newSession()}
              disabled={streaming || messages.length === 0}
              title={t('chat.newSession')}
              aria-label={t('chat.newSession')}
            >
              <SquarePen size={15} aria-hidden />
            </button>
          </>
        ) : (
          <button
            className={styles.iconBtn}
            onClick={() => void (tab === 'related' ? reloadRelated(path) : reloadSuggest(path))}
            title={t(tab === 'related' ? 'related.recompute' : 'suggest.recompute')}
            aria-label={t(tab === 'related' ? 'related.recompute' : 'suggest.recompute')}
          >
            <RefreshCw size={15} aria-hidden />
          </button>
        )}
        <button
          className={styles.iconBtn}
          onClick={() => closeChat()}
          title={t('chat.close')}
          aria-label={t('chat.close')}
        >
          <X size={15} aria-hidden />
        </button>
      </header>

      <div className={styles.tabs} role="tablist">
        <button
          role="tab"
          aria-selected={tab === 'chat'}
          className={`${styles.tab} ${tab === 'chat' ? styles.active : ''}`}
          onClick={() => setTab('chat')}
        >
          {t('chat.tabChat')}
        </button>
        <button
          role="tab"
          aria-selected={tab === 'suggest'}
          className={`${styles.tab} ${tab === 'suggest' ? styles.active : ''}`}
          onClick={() => setTab('suggest')}
        >
          {t('chat.tabSuggest')}
        </button>
        <button
          role="tab"
          aria-selected={tab === 'related'}
          className={`${styles.tab} ${tab === 'related' ? styles.active : ''}`}
          onClick={() => setTab('related')}
        >
          {t('chat.tabRelated')}
        </button>
      </div>

      <div className={styles.body}>
        {tab === 'chat' ? <ChatView /> : tab === 'related' ? <RelatedView /> : <SuggestView />}
      </div>
    </aside>
  );
}
