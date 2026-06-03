import { useEffect, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useTranslation } from 'react-i18next';

import { type ChatMessage, useChatStore } from '../../stores/chat';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import styles from './ChatPanel.module.css';

/**
 * Тело чата (Ф1-8 + виртуализация): лента сессии + композер. Оболочку (табы/закрытие) даёт `AiPanel`.
 * Лента виртуализирована (DESIGN §«лента виртуализирована»): рендерятся только видимые сообщения,
 * высота переменная → `measureElement`. Автоскролл к низу — только если пользователь уже у низа
 * (чтение истории не дёргается во время стрима). Стриминг токенов, источники-цитаты (→ открыть файл).
 */
export function ChatView() {
  const { t } = useTranslation();
  const messages = useChatStore((s) => s.messages);
  const streaming = useChatStore((s) => s.streaming);
  const send = useChatStore((s) => s.send);
  const stop = useChatStore((s) => s.stop);
  const center = useWorkspaceStore(activePath);
  const openFile = useWorkspaceStore((s) => s.openFile);

  const [input, setInput] = useState('');
  const feedRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);

  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => feedRef.current,
    estimateSize: () => 72,
    overscan: 6,
    // Стартовый размер — чтобы элементы рендерились до измерения (важно для jsdom-тестов).
    initialRect: { width: 360, height: 800 },
  });

  // Следим за низом при новом сообщении/токене (messages — новый ref на каждый патч стора).
  useEffect(() => {
    if (atBottom.current && messages.length > 0) {
      virtualizer.scrollToIndex(messages.length - 1, { align: 'end' });
    }
  }, [messages, virtualizer]);

  const onScroll = () => {
    const el = feedRef.current;
    if (el) atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  };

  const submit = () => {
    const q = input.trim();
    if (!q || streaming) return;
    atBottom.current = true; // свой вопрос → следим за ответом
    send(q, center ?? undefined);
    setInput('');
  };

  return (
    <>
      <div className={styles.feed} ref={feedRef} onScroll={onScroll}>
        {messages.length === 0 ? (
          <p className={styles.empty}>{t('chat.empty')}</p>
        ) : (
          <div style={{ height: `${virtualizer.getTotalSize()}px`, position: 'relative' }}>
            {virtualizer.getVirtualItems().map((vItem) => (
              <div
                key={messages[vItem.index].id}
                data-index={vItem.index}
                ref={virtualizer.measureElement}
                className={styles.row}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  width: '100%',
                  transform: `translateY(${vItem.start}px)`,
                }}
              >
                <Message message={messages[vItem.index]} onOpen={(p) => void openFile(p)} />
              </div>
            ))}
          </div>
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
    </>
  );
}

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
                  <button className={styles.source} onClick={() => onOpen(s.path)} title={s.snippet}>
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
