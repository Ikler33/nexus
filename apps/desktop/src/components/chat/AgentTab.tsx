import { useTranslation } from 'react-i18next';
import { ArrowUp, Hash, Inbox, Link2, Maximize2 } from 'lucide-react';
import { BrandThinking } from '../chrome/BrandThinking';
import { useUIStore } from '../../stores/ui';
import styles from './AiPanel.module.css';

/**
 * Вкладка «Castor» AI-панели (Hermes-6 `ai-panel.jsx` AgentTab): лаунчер агента — знак-орбита покоя +
 * имя Castor + описание + «Открыть раздел Агента» (полноэкранный agent-view) + быстрый старт. Реальный
 * запуск задач — в разделе Агента (подтверждение каждого шага); здесь только вход в него.
 */
export function AgentTab() {
  const { t } = useTranslation();
  const openAgent = useUIStore((s) => s.openAgent);
  // P1-11: каждый пункт «Быстрого старта» сидит СВОЙ промпт в композер агента (раньше все 3 звали
  // голый openAgent() → открывали агента, но поле оставалось пустым — 3 пункта неотличимы). Сид —
  // prefill (НЕ авто-отправка): пользователь правит и запускает сам (см. AgentView seed-consume).
  const acts: { icon: typeof Inbox; label: string; seed: string }[] = [
    { icon: Inbox, label: t('chat.castor.act1'), seed: t('chat.castor.seed1') },
    { icon: Link2, label: t('chat.castor.act2'), seed: t('chat.castor.seed2') },
    { icon: Hash, label: t('chat.castor.act3'), seed: t('chat.castor.seed3') },
  ];
  return (
    <div className={styles.agentLaunch}>
      <div className={styles.agentIntro}>
        <span className={styles.agentGlyph} aria-hidden>
          <BrandThinking size={26} className="idle" />
        </span>
        <div className={styles.agentName}>Castor</div>
        <div className={styles.agentDesc}>{t('chat.castor.desc')}</div>
      </div>
      <div className={styles.agentActs}>
        <button type="button" className={styles.agentOpen} onClick={() => openAgent()}>
          <Maximize2 size={15} aria-hidden />
          <span>{t('chat.castor.open')}</span>
        </button>
        <div className={styles.agentActsLabel}>{t('chat.castor.quickStart')}</div>
        {acts.map(({ icon: Ico, label, seed }) => (
          <button
            key={label}
            type="button"
            className={styles.agentAct}
            onClick={() => openAgent(seed)}
          >
            <Ico size={15} aria-hidden />
            <span>{label}</span>
            <ArrowUp size={13} aria-hidden className={styles.agentActGo} />
          </button>
        ))}
      </div>
    </div>
  );
}
