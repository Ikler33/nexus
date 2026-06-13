import { useEffect, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { AlertTriangle, FileText, Sparkles, Globe, ChevronRight, MessageSquare } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

import {
  disclosureOpen,
  type ChatMessage,
  type ChatMode,
  type ChatSource,
  useChatStore,
} from '../../stores/chat';
import { useUIStore } from '../../stores/ui';
import { tauriApi } from '../../lib/tauri-api';
import type { MemoryHit, WebSource } from '../../lib/tauri-api';
import { usePrefsStore } from '../../stores/prefs';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import { BrandThinking } from '../chrome/BrandThinking';
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
  const mode = useChatStore((s) => s.mode);
  const web = useChatStore((s) => s.web);
  const setMode = useChatStore((s) => s.setMode);
  const toggleWeb = useChatStore((s) => s.toggleWeb);
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

  // Клик по suggestion-pill в пустом состоянии — сразу отправляет готовый вопрос.
  const ask = (q: string) => {
    if (streaming) return;
    atBottom.current = true;
    send(q, center ?? undefined);
  };

  const pills = [t('chat.ask1'), t('chat.ask2'), t('chat.ask3')];

  return (
    <>
      <div className={styles.feed} ref={feedRef} onScroll={onScroll}>
        {messages.length === 0 ? (
          <div className={styles.emptyState}>
            <div className={styles.emptyGlyph} aria-hidden>
              <Sparkles size={24} />
            </div>
            <div className={styles.emptyTitle}>{t('chat.emptyTitle')}</div>
            <p className={styles.empty}>{t('chat.empty')}</p>
            <div className={styles.suggestPills}>
              {pills.map((p) => (
                <button
                  key={p}
                  type="button"
                  className={styles.suggestPill}
                  onClick={() => ask(p)}
                >
                  {p}
                </button>
              ))}
            </div>
          </div>
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

      {/* Web — ДОПОЛНИТЕЛЬНЫЙ флаг поверх режима (ревизия владельца 11.06): сегмент выбирает
          «По заметкам | Общий», глобус лишь разрешает модели сходить в интернет — режим не трогает. */}
      <div className={styles.modeRow}>
        <div role="radiogroup" aria-label={t('chat.mode')} className={styles.modeSeg}>
          {(['vault', 'general'] as const).map((m) => (
            <button
              key={m}
              type="button"
              role="radio"
              aria-checked={mode === m}
              className={`${styles.modeBtn} ${mode === m ? styles.modeOn : ''}`}
              onClick={() => setMode(m)}
              disabled={streaming}
              title={t(`chat.mode${m === 'vault' ? 'Vault' : 'General'}Hint`)}
            >
              {t(`chat.mode${m === 'vault' ? 'Vault' : 'General'}`)}
            </button>
          ))}
        </div>
        <button
          type="button"
          className={`${styles.webBtn} ${web ? styles.webOn : ''}`}
          aria-pressed={web}
          onClick={toggleWeb}
          disabled={streaming}
          title={t('chat.modeWebHint')}
        >
          <Globe size={13} aria-hidden />
          {t('chat.modeWeb')}
        </button>
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

      <div className={styles.composerFoot}>
        {streaming ? (
          <span className={styles.footStatus}>
            <span className={styles.footPulse} aria-hidden />
            {t(thinkingKey(mode, web))}
          </span>
        ) : (
          <span className={styles.footHint}>
            <kbd className={styles.kbd}>Shift</kbd>+<kbd className={styles.kbd}>↵</kbd>{' '}
            {t('chat.hintSend')}
          </span>
        )}
      </div>
    </>
  );
}

/** Имя заметки-источника: title или basename без `.md` (как везде после DP-15). */
function sourceName(s: ChatSource): string {
  return s.title ?? s.path.slice(s.path.lastIndexOf('/') + 1).replace(/\.md$/, '');
}

/**
 * RAG-источники в одном из трёх стилей макета `ai-panel.jsx` (DP-12, настройка
 * «Источники в чате»): cards (номер-плашка + заголовок + сниппет) / chips (пилюли) /
 * footnotes (сноски `[N]` под чертой).
 */
function Sources({ sources, onOpen }: { sources: ChatSource[]; onOpen: (path: string) => void }) {
  const { t } = useTranslation();
  const style = usePrefsStore((s) => s.ragSources);
  if (style === 'chips') {
    return (
      <div className={styles.srcChips} aria-label={t('chat.sources')}>
        {sources.map((s, i) => (
          <button
            key={s.chunkId}
            className={styles.srcChip}
            onClick={() => onOpen(s.path)}
            title={s.snippet}
          >
            <span className={styles.srcChipNum}>{i + 1}</span>
            <FileText size={12} aria-hidden />
            {sourceName(s)}
          </button>
        ))}
      </div>
    );
  }
  if (style === 'footnotes') {
    return (
      <div className={styles.srcFoot} aria-label={t('chat.sources')}>
        <div className={styles.srcFootHead}>{t('chat.sources')}</div>
        {sources.map((s, i) => (
          <button
            key={s.chunkId}
            className={styles.srcFootRow}
            onClick={() => onOpen(s.path)}
            title={s.snippet}
          >
            <span className={styles.srcFootNum}>[{i + 1}]</span>
            <span>{sourceName(s)}</span>
          </button>
        ))}
      </div>
    );
  }
  return (
    <div className={styles.srcCards} aria-label={t('chat.sources')}>
      {sources.map((s, i) => (
        <button key={s.chunkId} className={styles.srcCard} onClick={() => onOpen(s.path)}>
          <span className={styles.srcCardNum}>{i + 1}</span>
          <span className={styles.srcCardBody}>
            <span className={styles.srcCardTitle}>{sourceName(s)}</span>
            <span className={styles.srcCardCtx}>{s.snippet}</span>
          </span>
        </button>
      ))}
    </div>
  );
}

/**
 * Плавный вывод стрима (фидбэк 11.06, «айфон-стайл»): свежий чанк токенов появляется с лёгким
 * fade/blur. Во время стрима — плейн-текст (markdown по живому дёргал бы вёрстку), по завершении
 * Message переключается на markdown-рендер.
 */
function StreamingText({ text }: { text: string }) {
  const seen = useRef(0);
  const stable = text.slice(0, seen.current);
  const fresh = text.slice(seen.current);
  useEffect(() => {
    seen.current = text.length;
  });
  return (
    <>
      {stable}
      {fresh && (
        <span key={text.length} className={styles.fresh}>
          {fresh}
        </span>
      )}
    </>
  );
}

// Раскрытость аккордеонов — в disclosureOpen (стор): react-virtual размонтирует сообщения,
// ушедшие из вьюпорта, и useState сбрасывался — источники «сами сворачивались» при скролле.

/** Компактная плашка-аккордеон для источников (Sonnet-style, фидбэк 11.06): свернуто по умолчанию. */
function Disclosure({ id, label, children }: { id: string; label: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(() => disclosureOpen.get(id) ?? false);
  const toggle = () =>
    setOpen((o) => {
      if (disclosureOpen.size > 500) disclosureOpen.clear();
      disclosureOpen.set(id, !o);
      return !o;
    });
  return (
    <div className={styles.srcBox}>
      <button
        type="button"
        className={styles.srcToggle}
        aria-expanded={open}
        onClick={toggle}
      >
        <ChevronRight size={13} className={`${styles.chev} ${open ? styles.chevOpen : ''}`} aria-hidden />
        {label}
      </button>
      {open && children}
    </div>
  );
}

// Дефолтная фраза «думания» до первой сводки CoT — честная по режиму и web-флагу (баг 2026-06-11:
// в «Общем»/Web писало «Ищу по заметкам…», хотя ретрива по vault там нет).
const THINKING_KEY: Record<ChatMode, string> = {
  vault: 'chat.thinking',
  general: 'chat.thinkingPlain',
};
function thinkingKey(mode: ChatMode, web: boolean): string {
  return web ? 'chat.thinkingWeb' : THINKING_KEY[mode];
}

function Message({ message, onOpen }: { message: ChatMessage; onOpen: (path: string) => void }) {
  const { t } = useTranslation();
  // Режим заморожен на время стрима (setMode блокируется) → текущий режим честен для этого сообщения.
  const mode = useChatStore((s) => s.mode);
  const webFlag = useChatStore((s) => s.web);
  if (message.role === 'user') {
    return <div className={styles.user}>{message.content}</div>;
  }
  return (
    <div className={styles.assistant}>
      {message.deniedKind ? (
        // AC-EGR-14: типизированный отказ эгресса — i18n-баннер (макет .ai-banner.danger),
        // не сырая строка ошибки.
        <div className={styles.banner} role="alert">
          <AlertTriangle size={16} aria-hidden />
          <div>
            <div className={styles.bannerTitle}>{t(`chat.denied.${message.deniedKind}`)}</div>
            <div className={styles.bannerSub}>{t(`chat.denied.${message.deniedKind}Sub`)}</div>
            {message.deniedKind === 'notConfigured' && (
              <button
                type="button"
                className={styles.bannerAct}
                onClick={() => useUIStore.getState().openSettings('ai')}
              >
                {t('chat.denied.openSettings')}
              </button>
            )}
          </div>
        </div>
      ) : message.error ? (
        <p className={styles.error}>{t('chat.error', { message: message.error })}</p>
      ) : (
        <>
          {message.content ? (
            <div className={styles.answer}>
              {message.streaming ? (
                <>
                  <StreamingText text={message.content} />
                  <span className={styles.caret} aria-hidden />
                </>
              ) : (
                // LLM отвечает в markdown (фидбэк 11.06: «## выглядят не очень») → рендерим.
                <div className={styles.md}>
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>{message.content}</ReactMarkdown>
                </div>
              )}
            </div>
          ) : (
            message.streaming && (
              // Фаза размышления (DESIGN §msg-thinking): анимированный brand-mark + переливающийся
              // label. В label стримится живая сводка CoT (reasoningSummary); до первой сводки —
              // дефолтная фраза.
              <div className={styles.thinkingRow}>
                <BrandThinking size={28} />
                <span className={styles.thinkingLabel}>
                  {message.reasoningSummary || t(thinkingKey(mode, webFlag))}
                </span>
              </div>
            )
          )}
          {message.sources && message.sources.length > 0 && (
            <Disclosure
              id={`${message.id}:src`}
              label={t('chat.sourcesToggle', { count: message.sources.length })}
            >
              <Sources sources={message.sources} onOpen={onOpen} />
            </Disclosure>
          )}
          {message.webSources && message.webSources.length > 0 && (
            <Disclosure
              id={`${message.id}:web`}
              label={t('chat.webSourcesToggle', { count: message.webSources.length })}
            >
              <WebSources sources={message.webSources} />
            </Disclosure>
          )}
          {message.memorySources && message.memorySources.length > 0 && (
            <Disclosure
              id={`${message.id}:mem`}
              label={t('chat.memorySourcesToggle', { count: message.memorySources.length })}
            >
              <MemorySources sources={message.memorySources} />
            </Disclosure>
          )}
        </>
      )}
    </div>
  );
}

/** Память переписки (N4b): фрагменты прошлых диалогов — клик грузит ту сессию в ленту (как история).
 *  Это внутренние данные («второй мозг»), не внешний контент — открываем прямо в панели. */
function MemorySources({ sources }: { sources: MemoryHit[] }) {
  const { t } = useTranslation();
  const loadSession = useChatStore((s) => s.loadSession);
  return (
    <div className={styles.srcCards} aria-label={t('chat.memorySources')}>
      {sources.map((s, i) => (
        <button
          key={`${s.sessionId}:${i}`}
          type="button"
          className={styles.srcCard}
          onClick={() => void loadSession(s.sessionId)}
          title={t('chat.memoryOpen')}
        >
          <span className={styles.srcCardNum}>
            <MessageSquare size={12} aria-hidden />
          </span>
          <span className={styles.srcCardBody}>
            <span className={styles.srcCardTitle}>
              {s.sessionTitle} · {s.role === 'user' ? t('chat.memoryYou') : t('chat.memoryAi')}
            </span>
            <span className={styles.srcCardCtx}>{s.snippet}</span>
          </span>
        </button>
      ))}
    </div>
  );
}

/** Web-источники (W-3): карточки-цитаты с заголовком, доменом и сниппетом — ссылка открывается
 * во внешнем браузере (web-контент недоверенный, в приложение его не пускаем). */
function WebSources({ sources }: { sources: WebSource[] }) {
  const { t } = useTranslation();
  const host = (url: string) => {
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  };
  return (
    <div className={styles.srcCards} aria-label={t('chat.webSources')}>
      {sources.map((s, i) => (
        <a
          key={s.url + i}
          className={styles.srcCard}
          href={s.url}
          target="_blank"
          rel="noopener noreferrer"
          title={s.url}
          onClick={(e) => {
            // Tauri-вебвью не открывает target=_blank → системный браузер через opener.
            e.preventDefault();
            void tauriApi.external.open(s.url).catch(() => {});
          }}
        >
          <span className={styles.srcCardNum}>{i + 1}</span>
          <span className={styles.srcCardBody}>
            <span className={styles.srcCardTitle}>{s.title}</span>
            <span className={styles.srcCardCtx}>
              {host(s.url)} · {s.snippet}
            </span>
          </span>
        </a>
      ))}
    </div>
  );
}
