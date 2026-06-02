import { useEffect, useRef, useState } from 'react';
import { Trash2, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { type ChatMessage, useChatStore } from '../../stores/chat';
import { useUIStore } from '../../stores/ui';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import styles from './ChatPanel.module.css';

/**
 * Правая RAG-чат-панель (Ф1-8, DESIGN §«AI Chat»): лента сессии, стриминг токенов + «Стоп»,
 * ответ с кликабельными источниками (→ открыть файл). Контекст retrieval — открытый файл (граф-ранг).
 */
export function ChatPanel() {
  const { t } = useTranslation();
  const messages = useChatStore((s) => s.messages);
  const streaming = useChatStore((s) => s.streaming);
  const send = useChatStore((s) => s.send);
  const stop = useChatStore((s) => s.stop);
  const clear = useChatStore((s) => s.clear);
  const closeChat = useUIStore((s) => s.closeChat);
  const center = useWorkspaceStore(activePath);
  const openFile = useWorkspaceStore((s) => s.openFile);

  const [input, setInput] = useState('');
  const feedRef = useRef<HTMLDivElement>(null);

  // Автопрокрутка ленты к низу при новых сообщениях/токенах.
  useEffect(() => {
    const el = feedRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

  const submit = () => {
    const q = input.trim();
    if (!q || streaming) return;
    send(q, center ?? undefined);
    setInput('');
  };

  return (
    <aside className={styles.panel} aria-label={t('chat.title')}>
      <header className={styles.header}>
        <span className={styles.title}>{t('chat.title')}</span>
        <span className={styles.badge} title={t('chat.localHint')}>
          {t('chat.local')}
        </span>
        <div className={styles.actions}>
          <button
            className={styles.iconBtn}
            onClick={() => clear()}
            disabled={streaming || messages.length === 0}
            title={t('chat.clear')}
            aria-label={t('chat.clear')}
          >
            <Trash2 size={15} aria-hidden />
          </button>
          <button
            className={styles.iconBtn}
            onClick={() => closeChat()}
            title={t('chat.close')}
            aria-label={t('chat.close')}
          >
            <X size={15} aria-hidden />
          </button>
        </div>
      </header>

      <div className={styles.feed} ref={feedRef}>
        {messages.length === 0 ? (
          <p className={styles.empty}>{t('chat.empty')}</p>
        ) : (
          messages.map((m) => (
            <Message key={m.id} message={m} onOpen={(p) => void openFile(p)} />
          ))
        )}
      </div>

      <form
        className={styles.composer}
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <textarea
          className={styles.input}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          placeholder={t('chat.placeholder')}
          aria-label={t('chat.title')}
          rows={2}
        />
        {streaming ? (
          <button type="button" className={styles.stopBtn} onClick={() => stop()}>
            {t('chat.stop')}
          </button>
        ) : (
          <button type="submit" className={styles.sendBtn} disabled={!input.trim()}>
            {t('chat.send')}
          </button>
        )}
      </form>
    </aside>
  );
}

/** Одно сообщение ленты: вопрос пользователя или ответ ассистента (с источниками/ошибкой). */
function Message({ message, onOpen }: { message: ChatMessage; onOpen: (path: string) => void }) {
  const { t } = useTranslation();
  if (message.role === 'user') {
    return <div className={styles.user}>{message.content}</div>;
  }
  return (
    <div className={styles.assistant}>
      {message.error ? (
        <p className={styles.error}>{t('chat.error', { message: message.error })}</p>
      ) : (
        <>
          {message.content ? (
            <div className={styles.answer}>
              {message.content}
              {message.streaming && <span className={styles.caret} aria-hidden />}
            </div>
          ) : (
            message.streaming && <div className={styles.thinking}>{t('chat.thinking')}</div>
          )}
          {message.sources && message.sources.length > 0 && (
            <ul className={styles.sources} aria-label={t('chat.sources')}>
              {message.sources.map((s, i) => (
                <li key={s.chunkId}>
                  <button
                    className={styles.source}
                    onClick={() => onOpen(s.path)}
                    title={s.snippet}
                  >
                    <span className={styles.sourceIdx}>[{i + 1}]</span>
                    <span className={styles.sourcePath}>{s.title ?? s.path}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}
