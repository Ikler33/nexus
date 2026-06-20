import { useEffect, useState } from 'react';
import {
  AlertTriangle,
  Check,
  ChevronRight,
  Clock,
  Cpu,
  FlaskConical,
  FolderOpen,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { changeLocale } from '../../i18n/setup';
import { openVaultFlow } from '../../lib/commands-core';
import { isTauri, tauriApi } from '../../lib/tauri-api';
import { useThemeStore } from '../../stores/theme';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { BrandMark } from '../chrome/BrandMark';
import { BrandThinking } from '../chrome/BrandThinking';
import styles from './Onboarding.module.css';

type Step = 'welcome' | 'vault' | 'ai' | 'index';
type Health = 'none' | 'checking' | 'ok' | 'bad';

/** Индикатор шагов 1–3 (vault → AI → индексация), как в макете. */
function StepDots({ step }: { step: Step }) {
  const order: Step[] = ['vault', 'ai', 'index'];
  const idx = order.indexOf(step);
  if (idx < 0) return null;
  return (
    <div className={styles.steps} aria-hidden>
      {order.map((s, i) => (
        <span key={s} className={styles.stepWrap}>
          <i
            className={`${styles.stepDot} ${i < idx ? styles.stepDone : ''} ${i === idx ? styles.stepActive : ''}`}
          >
            {i < idx && <Check size={9} />}
          </i>
          {i < order.length - 1 && (
            <i className={`${styles.stepLine} ${i < idx ? styles.stepFilled : ''}`} />
          )}
        </span>
      ))}
    </div>
  );
}

/**
 * Онбординг (DP-7, макет `onboarding.jsx`): welcome → vault (системный диалог; в нём же можно
 * создать новую папку) → проверка AI (читает `.nexus/local.json` УЖЕ открытого vault, health
 * через `test_ai_connection`; не настроен — честно говорим и пускаем дальше) → индексация
 * (идёт фоном; «Готово» по событию `vault:changed`) → вход. Повторные запуски (флаг
 * `onboardingDone`) ведут с welcome сразу в диалог vault без шагов.
 */
export function Onboarding() {
  const { t, i18n } = useTranslation();
  const theme = useThemeStore((s) => s.theme);
  const toggleTheme = useThemeStore((s) => s.toggle);
  const lang = i18n.language === 'ru' ? 'ru' : 'en';
  const vaultOpen = useVaultStore((s) => s.info != null);
  const onboardingDone = useUIStore((s) => s.onboardingDone);
  const startOnboarding = useUIStore((s) => s.startOnboarding);
  const finishOnboarding = useUIStore((s) => s.finishOnboarding);

  const [step, setStep] = useState<Step>('welcome');
  const [health, setHealth] = useState<Health>('none');
  const [aiUrl, setAiUrl] = useState<string | null>(null);
  const [indexed, setIndexed] = useState(false);

  // Шаг AI: читаем конфиг открытого vault и пробуем эндпоинт chat-модели.
  useEffect(() => {
    if (step !== 'ai') return;
    let alive = true;
    void (async () => {
      try {
        const cfg = await tauriApi.settings.getAiConfig();
        const url = cfg.chat?.url ?? null;
        if (!alive) return;
        setAiUrl(url);
        if (!url) {
          setHealth('none');
          return;
        }
        setHealth('checking');
        await tauriApi.settings.testConnection(url);
        if (alive) setHealth('ok');
      } catch {
        if (alive) setHealth('bad');
      }
    })();
    return () => {
      alive = false;
    };
  }, [step]);

  // Шаг индексации: «Готово» по первому `vault:changed` (реиндекс завершён); вне Tauri — мок-таймер.
  useEffect(() => {
    if (step !== 'index') return;
    if (!isTauri()) {
      const timer = setTimeout(() => setIndexed(true), 1500);
      return () => clearTimeout(timer);
    }
    let unlisten = () => {};
    void tauriApi.events.onVaultChanged(() => setIndexed(true)).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten();
  }, [step]);

  const pickVault = async () => {
    await openVaultFlow();
    // Диалог могли отменить — шагаем дальше только если vault реально открыт.
    if (useVaultStore.getState().info) setStep('ai');
  };

  const healthPill = () => {
    switch (health) {
      case 'checking':
        return (
          <span className={`${styles.health} ${styles.healthChecking}`}>
            <BrandThinking size={14} />
            {t('onboarding.aiChecking')}
          </span>
        );
      case 'ok':
        return (
          <span className={`${styles.health} ${styles.healthOk}`}>
            <i className={styles.liveDot} />
            {t('onboarding.aiOk')}
          </span>
        );
      case 'bad':
        return (
          <span className={`${styles.health} ${styles.healthBad}`}>
            <AlertTriangle size={12} aria-hidden />
            {t('onboarding.aiBad')}
          </span>
        );
      default:
        return (
          <span className={styles.health}>
            <AlertTriangle size={12} aria-hidden />
            {t('onboarding.aiNone')}
          </span>
        );
    }
  };

  return (
    <div className={styles.screen}>
      <div className={`${styles.card} ${step !== 'welcome' ? styles.cardLeft : ''}`}>
        {step === 'welcome' && (
          <>
            <BrandMark size={76} />
            <div className={styles.eyebrow}>{t('onboarding.eyebrow')}</div>
            <h1 className={styles.title}>{t('app.name')}</h1>
            <p className={styles.sub}>{t('onboarding.sub')}</p>
            {onboardingDone ? (
              <button type="button" className={styles.cta} onClick={() => void openVaultFlow()}>
                <FolderOpen size={18} aria-hidden />
                {t('onboarding.openVault')}
              </button>
            ) : (
              <button
                type="button"
                className={styles.cta}
                onClick={() => {
                  startOnboarding();
                  setStep('vault');
                }}
              >
                {t('onboarding.start')}
                <ChevronRight size={16} aria-hidden />
              </button>
            )}
            {!onboardingDone && (
              <div className={styles.footHint}>
                <Clock size={12} aria-hidden />
                {t('onboarding.footHint')}
              </div>
            )}
            <div className={styles.controls}>
              <button
                type="button"
                className={styles.link}
                onClick={() => changeLocale(lang === 'ru' ? 'en' : 'ru')}
              >
                {lang === 'ru' ? 'English' : 'Русский'}
              </button>
              <span className={styles.dot}>·</span>
              <button type="button" className={styles.link} onClick={() => toggleTheme()}>
                {theme === 'dark' ? t('onboarding.light') : t('onboarding.dark')}
              </button>
            </div>
          </>
        )}

        {step === 'vault' && (
          <>
            <StepDots step={step} />
            <h2 className={styles.stepTitle}>{t('onboarding.vaultTitle')}</h2>
            <p className={styles.stepSub}>{t('onboarding.vaultSub')}</p>
            <div className={styles.optList}>
              <button type="button" className={styles.opt} onClick={() => void pickVault()}>
                <FolderOpen size={18} className={styles.optIco} aria-hidden />
                <span className={styles.optText}>
                  <span className={styles.optTitle}>{t('onboarding.vaultOpen')}</span>
                  <span className={styles.optSub}>{t('onboarding.vaultOpenSub')}</span>
                </span>
                <ChevronRight size={15} className={styles.optGo} aria-hidden />
              </button>
              {!isTauri() && (
                <button type="button" className={styles.opt} onClick={() => void pickVault()}>
                  <FlaskConical size={18} className={styles.optIco} aria-hidden />
                  <span className={styles.optText}>
                    <span className={styles.optTitle}>{t('onboarding.vaultDemo')}</span>
                    <span className={styles.optSub}>{t('onboarding.vaultDemoSub')}</span>
                  </span>
                  <ChevronRight size={15} className={styles.optGo} aria-hidden />
                </button>
              )}
            </div>
          </>
        )}

        {step === 'ai' && (
          <>
            <StepDots step={step} />
            <h2 className={styles.stepTitle}>{t('onboarding.aiTitle')}</h2>
            <div className={styles.aiRow}>
              <Cpu size={18} className={styles.optIco} aria-hidden />
              <span className={styles.optText}>
                <span className={styles.optTitle}>
                  {aiUrl ?? t('onboarding.aiNotConfigured')}
                </span>
                <span className={styles.optSub}>{t('onboarding.aiSub')}</span>
              </span>
              {healthPill()}
            </div>
            <p className={styles.note}>{t('onboarding.aiNote')}</p>
            <div className={styles.actions}>
              <button type="button" className={styles.ghost} onClick={() => setStep('vault')}>
                {t('onboarding.back')}
              </button>
              <button type="button" className={styles.cta} onClick={() => setStep('index')}>
                {t('onboarding.continue')}
                <ChevronRight size={16} aria-hidden />
              </button>
            </div>
          </>
        )}

        {step === 'index' && (
          <>
            <StepDots step={step} />
            <h2 className={styles.stepTitle}>
              {indexed ? t('onboarding.indexDone') : t('onboarding.indexTitle')}
            </h2>
            <p className={styles.stepSub}>
              {indexed ? t('onboarding.indexDoneSub') : t('onboarding.indexSub')}
            </p>
            <div className={styles.progress} aria-hidden>
              <i className={indexed ? styles.progressDone : styles.progressRun} />
            </div>
            <div className={styles.actions}>
              <button
                type="button"
                className={styles.cta}
                disabled={!vaultOpen}
                onClick={() => finishOnboarding()}
              >
                {t('onboarding.enter')}
                <ChevronRight size={16} aria-hidden />
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
