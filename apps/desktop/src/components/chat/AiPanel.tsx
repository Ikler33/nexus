import { useCallback, useEffect, useRef, useState } from 'react';
import {
  Check,
  FilePlus2,
  Search,
  HardDrive,
  History,
  Maximize2,
  SquarePen,
  WifiOff,
  X,
} from 'lucide-react';
import { OrbitIcon } from '../chrome/BrandGlyphs';
import { useTranslation } from 'react-i18next';

import { logUi } from '../../lib/debug-log';
import { tauriApi, type ChatSearchHit, type ChatSessionInfo } from '../../lib/tauri-api';
import { usePrefsStore } from '../../stores/prefs';
import { useChatStore } from '../../stores/chat';
import { useUIStore } from '../../stores/ui';
import { AgentTab } from './AgentTab';
import { ChatView } from './ChatView';
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
 * #58 (W-8, ревью): бэкенд-snippet оборачивает совпадения литеральными скобками `[...]` (FTS5
 * `snippet()`). Разбираем их в `<mark>` (CSP-safe, без innerHTML), убирая сами скобки.
 */
function renderSnippet(snippet: string, markClass: string) {
  return snippet.split(/\[([^\]]*)\]/g).map((part, i) =>
    i % 2 === 1 ? (
      <mark key={i} className={markClass}>
        {part}
      </mark>
    ) : (
      <span key={i}>{part}</span>
    ),
  );
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
  // #58 (W-8): полнотекстовый поиск по переписке.
  const [query, setQuery] = useState('');
  const [hits, setHits] = useState<ChatSearchHit[]>([]);
  // searched — поиск ПО ТЕКУЩЕМУ запросу уже завершился (ревью: иначе «Совпадений нет» мигает до дебаунса).
  const [searched, setSearched] = useState(false);
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

  // #58: дебаунс-поиск по переписке (пустой запрос → показываем обычную историю по бакетам).
  useEffect(() => {
    const q = query.trim();
    setSearched(false); // запрос изменился — результат ещё не получен
    if (!q) {
      setHits([]);
      return;
    }
    let alive = true;
    const timer = setTimeout(() => {
      void tauriApi.chat.sessions
        .search(q, 50)
        .then((h) => {
          if (alive) {
            setHits(h);
            setSearched(true);
          }
        })
        .catch(() => {
          if (alive) {
            setHits([]);
            setSearched(true);
          }
        });
    }, 220);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [query]);

  // Сброс поиска при закрытии меню.
  useEffect(() => {
    if (!open) setQuery('');
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
          {/* #58 (W-8): поиск по переписке. */}
          <div className={styles.histSearch}>
            <Search size={13} aria-hidden />
            <input
              type="text"
              className={styles.histSearchInput}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t('chat.searchPlaceholder')}
              aria-label={t('chat.searchSessions')}
              spellCheck={false}
            />
          </div>
          {/* Поиск активен → результаты-совпадения; иначе — обычная история по бакетам. */}
          {query.trim() ? (
            hits.length > 0 ? (
              hits.map((h, i) => (
                <button
                  key={`${h.sessionId}-${h.createdAt}-${i}`}
                  type="button"
                  role="menuitem"
                  className={styles.histHit}
                  onClick={() => {
                    setOpen(false);
                    logUi('chat:search-load-session', String(h.sessionId));
                    void loadSession(h.sessionId);
                  }}
                >
                  <span className={styles.histHitTitle}>{h.title}</span>
                  <span className={styles.histHitSnip}>
                    {renderSnippet(h.snippet, styles.snipMark)}
                  </span>
                </button>
              ))
            ) : searched ? (
              // Пусто показываем ТОЛЬКО после завершения поиска (ревью: иначе мигает до дебаунса).
              <div className={styles.histEmpty}>{t('chat.searchEmpty')}</div>
            ) : null
          ) : (
            <>
              {buckets.length === 0 && (
                <div className={styles.histEmpty}>{t('chat.historyEmpty')}</div>
              )}
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
            </>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * AI-панель Castor (Hermes-6 `ai-panel.jsx`): шапка (орбита-глиф + «Castor» + бейдж провайдера +
 * история/новая сессия + развернуть-в-раздел + закрыть), две вкладки — «Чат» (RAG, Ф1-8) и «Castor»
 * (лаунчер раздела Агента). «Связи»/«Похожие» переехали в инспектор-рейл редактора (per-заметочные),
 * суммаризация — в inline-LLM редактора.
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

  const openAgent = useUIStore((s) => s.openAgent);

  // Драг-ресайз панели (фидбэк владельца 11.06): тянем левую кромку (side) / верхнюю (bottom);
  // размер живёт в prefs (персист) и применяется грид-переменной App. Overlay не ресайзим.
  const setW = usePrefsStore((s) => s.setAiPanelW);
  const setH = usePrefsStore((s) => s.setAiPanelH);
  // Контроллер активного драга: один abort() снимает оба window-слушателя (через signal). Нужен,
  // чтобы размонтирование панели ВО ВРЕМЯ перетаскивания не оставило висячие mousemove/mouseup на
  // window (onUp тогда не сработал бы) — утечка слушателей со stale-замыканием (находка аудита B11).
  const resizeAbort = useRef<AbortController | null>(null);
  const startResize = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      resizeAbort.current?.abort(); // подстраховка от незавершённого предыдущего драга
      const ctrl = new AbortController();
      resizeAbort.current = ctrl;
      const horizontal = variant === 'side';
      const onMove = (ev: MouseEvent) => {
        if (horizontal) setW(window.innerWidth - ev.clientX);
        else setH(window.innerHeight - ev.clientY);
      };
      const onUp = () => {
        ctrl.abort();
        resizeAbort.current = null;
        document.body.style.cursor = '';
      };
      document.body.style.cursor = horizontal ? 'col-resize' : 'row-resize';
      window.addEventListener('mousemove', onMove, { signal: ctrl.signal });
      window.addEventListener('mouseup', onUp, { signal: ctrl.signal });
    },
    [variant, setW, setH],
  );
  // Размонтирование во время драга — снимаем слушатели и возвращаем курсор.
  useEffect(
    () => () => {
      if (resizeAbort.current) {
        resizeAbort.current.abort();
        document.body.style.cursor = '';
      }
    },
    [],
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
          <OrbitIcon size={16} aria-hidden />
          {t('chat.title2')}
        </span>
        <span className={styles.headSpacer} />
        <ProviderBadge />
        {tab === 'chat' && (
          // Решение владельца 2026-06-12: ничего не удаляем — «История» + «Новая сессия» (текущая
          // лента уходит в память «второго мозга», не стирается).
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
        )}
        <button
          className={styles.iconBtn}
          onClick={() => openAgent()}
          title={t('chat.openAgentSection')}
          aria-label={t('chat.openAgentSection')}
        >
          <Maximize2 size={15} aria-hidden />
        </button>
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
          aria-selected={tab === 'agent'}
          className={`${styles.tab} ${tab === 'agent' ? styles.active : ''}`}
          onClick={() => setTab('agent')}
        >
          Castor
        </button>
      </div>

      <div className={styles.body}>{tab === 'chat' ? <ChatView /> : <AgentTab />}</div>
    </aside>
  );
}
