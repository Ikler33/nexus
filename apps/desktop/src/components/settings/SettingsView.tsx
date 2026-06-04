import { useEffect, useState } from 'react';
import { Cpu, Info, Keyboard, Palette, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi } from '../../lib/tauri-api';
import { ACCENTS, useThemeStore } from '../../stores/theme';
import type { Accent } from '../../stores/theme';
import { useUIStore } from '../../stores/ui';
import type { SettingsSection } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import styles from './SettingsView.module.css';

/** Превью-цвет свотча акцента (реальный акцент — data-accent в токенах). */
const ACCENT_PREVIEW: Record<Accent, string> = {
  amber: 'oklch(0.62 0.135 47)',
  teal: 'oklch(0.6 0.08 205)',
  sage: 'oklch(0.6 0.07 158)',
  clay: 'oklch(0.58 0.11 28)',
};

const SECTIONS: { id: SettingsSection; icon: typeof Palette; key: string }[] = [
  { id: 'appearance', icon: Palette, key: 'settings.appearance' },
  { id: 'ai', icon: Cpu, key: 'settings.ai' },
  { id: 'hotkeys', icon: Keyboard, key: 'settings.hotkeys' },
  { id: 'about', icon: Info, key: 'settings.about' },
];

/**
 * Раздел настроек (кросс-план #11, по образцу Obsidian): модалка с левым навом секций + контент-панель.
 * Секции собираются инкрементально (слайсами): «Оформление» и «О программе» — здесь; «AI / Модели» и
 * «Горячие клавиши» — следующими срезами (сейчас заглушки). Состояние открытия/секции — в ui-сторе.
 */
export function SettingsView() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeTweaks);
  const section = useUIStore((s) => s.settingsSection);
  const setSection = useUIStore((s) => s.setSettingsSection);

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        className={styles.modal}
        role="dialog"
        aria-modal="true"
        aria-label={t('settings.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <nav className={styles.nav} aria-label={t('settings.title')}>
          <div className={styles.navTitle}>{t('settings.title')}</div>
          {SECTIONS.map((s) => (
            <button
              key={s.id}
              type="button"
              className={`${styles.navItem} ${section === s.id ? styles.navOn : ''}`}
              onClick={() => setSection(s.id)}
              aria-current={section === s.id}
            >
              <s.icon size={15} aria-hidden />
              <span>{t(s.key)}</span>
            </button>
          ))}
        </nav>

        <div className={styles.content}>
          <button type="button" className={styles.close} onClick={close} aria-label={t('git.close')}>
            <X size={16} aria-hidden />
          </button>
          {section === 'appearance' && <AppearanceSection />}
          {section === 'ai' && <Stub text={t('settings.soon')} />}
          {section === 'hotkeys' && <Stub text={t('settings.soon')} />}
          {section === 'about' && <AboutSection />}
        </div>
      </div>
    </div>
  );
}

function AppearanceSection() {
  const { t } = useTranslation();
  const theme = useThemeStore((s) => s.theme);
  const setTheme = useThemeStore((s) => s.setTheme);
  const accent = useThemeStore((s) => s.accent);
  const setAccent = useThemeStore((s) => s.setAccent);
  const density = useThemeStore((s) => s.density);
  const setDensity = useThemeStore((s) => s.setDensity);

  return (
    <>
      <h2 className={styles.h2}>{t('settings.appearance')}</h2>
      <section className={styles.group}>
        <span className={styles.label}>{t('tweaks.theme')}</span>
        <div className={styles.seg}>
          <button
            type="button"
            className={`${styles.segBtn} ${theme === 'light' ? styles.on : ''}`}
            onClick={() => setTheme('light')}
          >
            {t('tweaks.light')}
          </button>
          <button
            type="button"
            className={`${styles.segBtn} ${theme === 'dark' ? styles.on : ''}`}
            onClick={() => setTheme('dark')}
          >
            {t('tweaks.dark')}
          </button>
        </div>
      </section>
      <section className={styles.group}>
        <span className={styles.label}>{t('tweaks.accent')}</span>
        <div className={styles.swatches}>
          {ACCENTS.map((a) => (
            <button
              key={a}
              type="button"
              className={`${styles.swatch} ${accent === a ? styles.swatchOn : ''}`}
              style={{ background: ACCENT_PREVIEW[a] }}
              onClick={() => setAccent(a)}
              aria-label={a}
              aria-pressed={accent === a}
            />
          ))}
        </div>
      </section>
      <section className={styles.group}>
        <span className={styles.label}>{t('tweaks.density')}</span>
        <div className={styles.seg}>
          <button
            type="button"
            className={`${styles.segBtn} ${density === 'comfortable' ? styles.on : ''}`}
            onClick={() => setDensity('comfortable')}
          >
            {t('tweaks.comfortable')}
          </button>
          <button
            type="button"
            className={`${styles.segBtn} ${density === 'compact' ? styles.on : ''}`}
            onClick={() => setDensity('compact')}
          >
            {t('tweaks.compact')}
          </button>
        </div>
      </section>
    </>
  );
}

function AboutSection() {
  const { t } = useTranslation();
  const vaultRoot = useVaultStore((s) => s.info?.root ?? null);
  const [version, setVersion] = useState('—');
  useEffect(() => {
    let alive = true;
    void tauriApi.app.version().then((v) => {
      if (alive) setVersion(v);
    });
    return () => {
      alive = false;
    };
  }, []);

  return (
    <>
      <h2 className={styles.h2}>{t('settings.about')}</h2>
      <dl className={styles.about}>
        <dt>{t('settings.app')}</dt>
        <dd>Nexus</dd>
        <dt>{t('settings.version')}</dt>
        <dd className={styles.mono}>{version}</dd>
        <dt>{t('settings.vault')}</dt>
        <dd className={styles.mono}>{vaultRoot ?? t('settings.noVault')}</dd>
      </dl>
    </>
  );
}

function Stub({ text }: { text: string }) {
  return <p className={styles.stub}>{text}</p>;
}
