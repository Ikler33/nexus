import { useEffect, useRef, useState } from 'react';
import { ChevronLeft, ChevronRight, Plus } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { BrandThinking } from '../common/BrandThinking';
import { useAgentStore, sessionStatus } from '../../stores/agent';
import { tauriApi, type AgentSessionInfo } from '../../lib/tauri-api';
import styles from './AgentHistory.module.css';

const COLLAPSED_KEY = 'agent.history.collapsed';

function readCollapsed(): boolean {
  try {
    return localStorage.getItem(COLLAPSED_KEY) === '1';
  } catch {
    return false; // localStorage недоступен (node/test) — дефолт развёрнут
  }
}

function persistCollapsed(v: boolean): void {
  try {
    localStorage.setItem(COLLAPSED_KEY, v ? '1' : '0');
  } catch {
    /* недоступен — игнор */
  }
}

/**
 * W-38: ЛЕВЫЙ САЙДБАР истории переписок агента (вкладка Castor). Грузит список агент-сессий
 * (`agent_sessions_list`) на mount и ПОСЛЕ завершения активного хода (переход active→терминал); клик по
 * строке переоткрывает переписку (`loadSession`); активная строка = `currentSessionId`. Сверху —
 * «Новая переписка» (`newSession`). Сворачивается (ширина → тонкий ре-открыватель); состояние
 * свёрнутости персистится в localStorage.
 */
export function AgentHistory() {
  const { t, i18n } = useTranslation();
  const currentSessionId = useAgentStore((s) => s.currentSessionId);
  const loadSession = useAgentStore((s) => s.loadSession);
  const newSession = useAgentStore((s) => s.newSession);
  // Статус сессии — для перезагрузки списка при переходе active→done (свежеперсистированный ход).
  const status = useAgentStore((s) => sessionStatus(s.turns));

  const [sessions, setSessions] = useState<AgentSessionInfo[]>([]);
  const [collapsed, setCollapsed] = useState(readCollapsed);
  const wasActive = useRef(false);

  const refresh = () => {
    void tauriApi.agent.sessions
      .list()
      .then(setSessions)
      .catch(() => {
        /* нет vault / ошибка — список не трогаем */
      });
  };

  // Загрузка на mount.
  useEffect(() => {
    refresh();
  }, []);

  // Перезагрузка после завершения активного хода (персист случился на терминале прогона).
  const active = status === 'running' || status === 'paused' || status === 'awaiting';
  useEffect(() => {
    if (wasActive.current && !active) refresh();
    wasActive.current = active;
  }, [active]);

  const toggleCollapsed = () => {
    setCollapsed((c) => {
      const next = !c;
      persistCollapsed(next);
      return next;
    });
  };

  const relTime = (sec: number) => {
    const diff = Math.floor(Date.now() / 1000) - sec;
    const ru = i18n.language === 'ru';
    if (diff < 60) return ru ? 'только что' : 'just now';
    if (diff < 3600) {
      const m = Math.floor(diff / 60);
      return ru ? `${m} мин назад` : `${m}m ago`;
    }
    if (diff < 86_400) {
      const h = Math.floor(diff / 3600);
      return ru ? `${h} ч назад` : `${h}h ago`;
    }
    const d = Math.floor(diff / 86_400);
    if (d < 7) return ru ? `${d} дн назад` : `${d}d ago`;
    return new Date(sec * 1000).toLocaleDateString(ru ? 'ru-RU' : 'en-US', {
      day: 'numeric',
      month: 'short',
    });
  };

  if (collapsed) {
    return (
      <button
        type="button"
        className={styles.reopen}
        onClick={toggleCollapsed}
        title={t('agent.history.expand')}
        aria-label={t('agent.history.expand')}
      >
        <ChevronRight size={15} aria-hidden />
      </button>
    );
  }

  return (
    <aside className={styles.histbar} aria-label={t('agent.history.title')}>
      <div className={styles.head}>
        <span className={styles.headT}>{t('agent.history.title')}</span>
        <button
          type="button"
          className={styles.headBtn}
          onClick={toggleCollapsed}
          title={t('agent.history.collapse')}
          aria-label={t('agent.history.collapse')}
        >
          <ChevronLeft size={15} aria-hidden />
        </button>
      </div>

      <button
        type="button"
        className={styles.newBtn}
        onClick={() => newSession()}
        title={t('agent.history.new')}
      >
        <Plus size={14} aria-hidden />
        {t('agent.history.new')}
      </button>

      <div className={styles.list}>
        {sessions.length === 0 ? (
          <div className={styles.empty}>{t('agent.history.empty')}</div>
        ) : (
          sessions.map((s) => (
            <button
              type="button"
              key={s.sessionId}
              className={`${styles.row} ${s.sessionId === currentSessionId ? styles.rowActive : ''}`}
              onClick={() => void loadSession(s.sessionId)}
              title={s.title}
              aria-current={s.sessionId === currentSessionId}
            >
              <span
                className={`${styles.dot} ${s.status === 'error' ? styles.dotErr : styles.dotDone}`}
                aria-hidden
              />
              <span className={styles.rowMain}>
                <span className={styles.rowTitle}>{s.title}</span>
                <span className={styles.rowMeta}>
                  {relTime(s.updatedAt)} · {t('agent.history.turns', { count: s.turnCount })}
                </span>
              </span>
            </button>
          ))
        )}
      </div>

      <div className={styles.foot}>
        <BrandThinking size={11} animate={false} />
        <span>{t('agent.history.title')}</span>
      </div>
    </aside>
  );
}
