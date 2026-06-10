import { useEffect, useState } from 'react';
import { HardDrive, RefreshCw, Sparkles, Trash2, WifiOff, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi } from '../../lib/tauri-api';
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
  const clearChat = useChatStore((s) => s.clear);

  const reloadSuggest = useSuggestStore((s) => s.load);
  const reloadRelated = useRelatedStore((s) => s.load);
  const path = useWorkspaceStore(activePath);

  return (
    <aside className={panelClass} aria-label={t('chat.title2')}>
      <header className={styles.head}>
        <span className={styles.headTitle}>
          <Sparkles size={16} aria-hidden />
          {t('chat.title2')}
        </span>
        <span className={styles.headSpacer} />
        <ProviderBadge />
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
