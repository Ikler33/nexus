import { useEffect } from 'react';
import { History, RotateCcw, Trash2, EyeOff, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFocusTrap } from '../../hooks/useFocusTrap';
import { useEpisodeStore } from '../../stores/episode';
import { useChatStore } from '../../stores/chat';
import { useToastStore } from '../../stores/toast';
import { useUIStore } from '../../stores/ui';
import { BrandThinking } from '../common/BrandThinking';
import styles from './EpisodesPanel.module.css';

/**
 * Панель «Эпизоды» (EP-3; спека `docs/specs/agent-episodic-memory.md` §9): таймлайн саммари
 * завершённых чат-сессий (обратная хронология). Клик по карточке грузит сессию в ленту чата. «Скрыть»
 * (обратимо, undo-тост) убирает эпизод из ретривала; «Удалить навсегда» (подтверждение) стирает строку
 * и вектор. focus-trap-модалка «как Память ИИ» (Esc/клик-вне закрывают).
 */
export function EpisodesPanel() {
  const { t, i18n } = useTranslation();
  const close = useUIStore((s) => s.closeEpisodes);
  const trapRef = useFocusTrap<HTMLDivElement>(close);
  const episodes = useEpisodeStore((s) => s.episodes);
  const loading = useEpisodeStore((s) => s.loading);
  const load = useEpisodeStore((s) => s.load);
  const dismiss = useEpisodeStore((s) => s.dismiss);
  const restore = useEpisodeStore((s) => s.restore);
  const purge = useEpisodeStore((s) => s.purge);
  const loadSession = useChatStore((s) => s.loadSession);

  useEffect(() => {
    void load();
  }, [load]);

  const fmtRange = (startedAt: number, endedAt: number) => {
    const f = (sec: number) =>
      new Date(sec * 1000).toLocaleDateString(i18n.language === 'ru' ? 'ru-RU' : 'en-US', {
        day: 'numeric',
        month: 'short',
      });
    const a = f(startedAt);
    const b = f(endedAt);
    return a === b ? b : `${a} – ${b}`;
  };

  const openSession = (sessionId: number) => {
    void loadSession(sessionId);
    close();
  };

  const onDismiss = (id: number) => {
    void dismiss(id);
    useToastStore.getState().addToast(t('episode.dismissed'), {
      kind: 'info',
      action: { label: t('episode.undo'), run: () => void restore(id) },
    });
  };

  const onPurge = (id: number, title: string) => {
    if (window.confirm(t('episode.purgeConfirm', { title }))) void purge(id);
  };

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        ref={trapRef}
        tabIndex={-1}
        className={styles.panel}
        role="dialog"
        aria-modal="true"
        aria-label={t('episode.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <History size={16} aria-hidden />
          <span className={styles.title}>{t('episode.title')}</span>
          {episodes.length > 0 && <span className={styles.count}>{episodes.length}</span>}
          <span className={styles.spacer} />
          <button
            className={styles.iconBtn}
            onClick={close}
            title={t('episode.close')}
            aria-label={t('episode.close')}
          >
            <X size={15} aria-hidden />
          </button>
        </header>

        {loading && episodes.length === 0 ? (
          <div className={styles.thinking}>
            <BrandThinking size={26} />
            <span className="mt-label">{t('episode.loading')}</span>
          </div>
        ) : episodes.length === 0 ? (
          <div className={styles.emptyState}>
            <History size={22} className={styles.emptyIco} aria-hidden />
            <p className={styles.empty}>{t('episode.empty')}</p>
          </div>
        ) : (
          <ul className={styles.list}>
            {episodes.map((e) => (
              <li key={e.id} className={`${styles.card} ${e.dismissed ? styles.dismissedCard : ''}`}>
                <div className={styles.cardHead}>
                  <span className={styles.date}>{fmtRange(e.startedAt, e.endedAt)}</span>
                  <button
                    type="button"
                    className={styles.cardTitle}
                    onClick={() => openSession(e.sessionId)}
                    title={t('episode.openSession')}
                  >
                    {e.sessionTitle}
                  </button>
                  {e.dismissed && <span className={styles.hiddenBadge}>{t('episode.hidden')}</span>}
                  <span className={styles.cardActions}>
                    {e.dismissed ? (
                      <button
                        type="button"
                        className={styles.actBtn}
                        onClick={() => void restore(e.id)}
                        title={t('episode.restore')}
                        aria-label={t('episode.restore')}
                      >
                        <RotateCcw size={14} aria-hidden />
                      </button>
                    ) : (
                      <button
                        type="button"
                        className={styles.actBtn}
                        onClick={() => onDismiss(e.id)}
                        title={t('episode.dismiss')}
                        aria-label={t('episode.dismiss')}
                      >
                        <EyeOff size={14} aria-hidden />
                      </button>
                    )}
                    <button
                      type="button"
                      className={`${styles.actBtn} ${styles.delBtn}`}
                      onClick={() => onPurge(e.id, e.sessionTitle)}
                      title={t('episode.purge')}
                      aria-label={t('episode.purge')}
                    >
                      <Trash2 size={14} aria-hidden />
                    </button>
                  </span>
                </div>
                <p className={styles.summary}>{e.summary}</p>
                {e.topics.length > 0 && (
                  <div className={styles.topics}>
                    {e.topics.map((tp, i) => (
                      <span key={`${tp}:${i}`} className={styles.topic}>
                        {tp}
                      </span>
                    ))}
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
