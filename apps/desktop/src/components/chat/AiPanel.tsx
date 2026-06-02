import { RefreshCw, Trash2, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useChatStore } from '../../stores/chat';
import { useSuggestStore } from '../../stores/suggest';
import { useUIStore } from '../../stores/ui';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import { ChatView } from './ChatView';
import { SuggestView } from './SuggestView';
import styles from './AiPanel.module.css';

/**
 * Правая AI-панель (DESIGN §ai-panel): вкладки «Чат» (RAG, Ф1-8) и «Связи» (предложения, Ф1-9).
 * Оболочка владеет табами, бейджем «локально», контекстным действием (очистить/пересчитать) и закрытием.
 */
export function AiPanel() {
  const { t } = useTranslation();
  const tab = useUIStore((s) => s.aiTab);
  const setTab = useUIStore((s) => s.setAiTab);
  const closeChat = useUIStore((s) => s.closeChat);

  const messages = useChatStore((s) => s.messages);
  const streaming = useChatStore((s) => s.streaming);
  const clearChat = useChatStore((s) => s.clear);

  const reloadSuggest = useSuggestStore((s) => s.load);
  const path = useWorkspaceStore(activePath);

  return (
    <aside className={styles.panel} aria-label={t('chat.title')}>
      <header className={styles.tabbar}>
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
        </div>

        <span className={styles.badge} title={t('chat.localHint')}>
          {t('chat.local')}
        </span>

        {tab === 'chat' ? (
          <button
            className={styles.iconBtn}
            onClick={() => clearChat()}
            disabled={streaming || messages.length === 0}
            title={t('chat.clear')}
            aria-label={t('chat.clear')}
          >
            <Trash2 size={15} aria-hidden />
          </button>
        ) : (
          <button
            className={styles.iconBtn}
            onClick={() => void reloadSuggest(path)}
            title={t('suggest.recompute')}
            aria-label={t('suggest.recompute')}
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

      <div className={styles.body}>{tab === 'chat' ? <ChatView /> : <SuggestView />}</div>
    </aside>
  );
}
