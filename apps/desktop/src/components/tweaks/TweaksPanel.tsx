import { useTranslation } from 'react-i18next';
import { ACCENTS, useThemeStore } from '../../stores/theme';
import type { Accent } from '../../stores/theme';
import { useUIStore } from '../../stores/ui';
import styles from './TweaksPanel.module.css';

/** Превью-цвет свотча акцента (для кнопок-выбора; реальный акцент задаётся data-accent в токенах). */
const ACCENT_PREVIEW: Record<Accent, string> = {
  amber: 'oklch(0.62 0.135 47)',
  teal: 'oklch(0.6 0.08 205)',
  sage: 'oklch(0.6 0.07 158)',
  clay: 'oklch(0.58 0.11 28)',
};

/**
 * Панель настроек оформления (Ф4-12): тема (light/dark), акцент (amber/teal/sage/clay → data-accent),
 * плотность (--row-h). Всё применяется мгновенно через theme-стор (persist + кросс-фейд).
 */
export function TweaksPanel() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeTweaks);
  const theme = useThemeStore((s) => s.theme);
  const setTheme = useThemeStore((s) => s.setTheme);
  const accent = useThemeStore((s) => s.accent);
  const setAccent = useThemeStore((s) => s.setAccent);
  const density = useThemeStore((s) => s.density);
  const setDensity = useThemeStore((s) => s.setDensity);

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        className={styles.panel}
        role="dialog"
        aria-label={t('tweaks.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          <span className={styles.title}>{t('tweaks.title')}</span>
          <button type="button" className={styles.close} onClick={close} aria-label={t('git.close')}>
            ✕
          </button>
        </header>
        <div className={styles.body}>
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
        </div>
      </div>
    </div>
  );
}
