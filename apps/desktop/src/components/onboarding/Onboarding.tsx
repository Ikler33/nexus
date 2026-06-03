import { FolderOpen } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { changeLocale } from '../../i18n/setup';
import { openVaultFlow } from '../../lib/commands-core';
import { useThemeStore } from '../../stores/theme';
import { BrandMark } from '../chrome/BrandMark';
import styles from './Onboarding.module.css';

/**
 * Первый запуск (vault не открыт): приветственный экран дизайн-системы. Бренд + интро + CTA
 * «Открыть vault» (нативный диалог в Tauri / мок в браузере) + переключатели языка и темы.
 * Многошаговый flow (проверка LLM-сервера, прогресс первичной индексации) — рефайнмент (BACKLOG):
 * индексация и так идёт фоном после открытия vault.
 */
export function Onboarding() {
  const { t, i18n } = useTranslation();
  const theme = useThemeStore((s) => s.theme);
  const toggleTheme = useThemeStore((s) => s.toggle);
  const lang = i18n.language === 'ru' ? 'ru' : 'en';

  return (
    <div className={styles.screen}>
      <div className={styles.card}>
        <BrandMark size={56} />
        <h1 className={styles.title}>{t('onboarding.welcome')}</h1>
        <p className={styles.sub}>{t('onboarding.sub')}</p>
        <button type="button" className={styles.cta} onClick={() => void openVaultFlow()}>
          <FolderOpen size={18} aria-hidden />
          {t('onboarding.openVault')}
        </button>
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
      </div>
    </div>
  );
}
