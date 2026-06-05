import {
  FolderOpen,
  GitBranch,
  MessageSquare,
  Moon,
  Newspaper,
  Puzzle,
  Search,
  Share2,
  SlidersHorizontal,
  Sun,
  Target,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { changeLocale } from '../../i18n/setup';
import { openVaultFlow } from '../../lib/commands-core';
import { useThemeStore } from '../../stores/theme';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { BrandMark } from './BrandMark';
import styles from './Titlebar.module.css';

/**
 * Верхний titlebar дизайн-системы (Liquid-Glass): бренд-марк + имя vault, центральная
 * поисковая пилюля (открывает Command Palette, ⌘K) и правая группа инструментов
 * (чат / граф / плагины / sync · тема / язык / открыть vault). Вариант A — кастомный
 * бар внутри обычного OS-окна (без frameless/traffic-lights; включим отдельным шагом).
 */
export function Titlebar() {
  const { t, i18n } = useTranslation();
  const info = useVaultStore((s) => s.info);
  const openPalette = useUIStore((s) => s.openPalette);
  const chatOpen = useUIStore((s) => s.chatOpen);
  const toggleChat = useUIStore((s) => s.toggleChat);
  const toggleGraph = useUIStore((s) => s.toggleGraph);
  const pluginsOpen = useUIStore((s) => s.pluginsOpen);
  const togglePlugins = useUIStore((s) => s.togglePlugins);
  const syncOpen = useUIStore((s) => s.syncOpen);
  const toggleSync = useUIStore((s) => s.toggleSync);
  const goalsOpen = useUIStore((s) => s.goalsOpen);
  const toggleGoals = useUIStore((s) => s.toggleGoals);
  const digestOpen = useUIStore((s) => s.digestOpen);
  const toggleDigest = useUIStore((s) => s.toggleDigest);
  const tweaksOpen = useUIStore((s) => s.tweaksOpen);
  const toggleTweaks = useUIStore((s) => s.toggleTweaks);
  const theme = useThemeStore((s) => s.theme);
  const toggleTheme = useThemeStore((s) => s.toggle);
  const lang = i18n.language === 'ru' ? 'ru' : 'en';

  return (
    <div className={styles.titlebar}>
      <div className={styles.brand}>
        <BrandMark size={24} />
        <span className={styles.appName}>{info?.name ?? t('app.name')}</span>
      </div>

      <span className={styles.spacer} />
      <button type="button" className={styles.search} onClick={() => openPalette()}>
        <Search size={14} aria-hidden />
        <span>{t('chrome.search')}</span>
        <kbd className={styles.kbd}>⌘K</kbd>
      </button>
      <span className={styles.spacer} />

      <div className={styles.group}>
        <button
          type="button"
          className={`${styles.tbBtn} ${chatOpen ? styles.active : ''}`}
          onClick={() => toggleChat()}
          title={t('commands.view.chat')}
          aria-label={t('commands.view.chat')}
          aria-pressed={chatOpen}
        >
          <MessageSquare size={16} aria-hidden />
        </button>
        <button
          type="button"
          className={styles.tbBtn}
          onClick={() => toggleGraph()}
          title={t('commands.view.graph')}
          aria-label={t('commands.view.graph')}
        >
          <Share2 size={16} aria-hidden />
        </button>
        <button
          type="button"
          className={`${styles.tbBtn} ${pluginsOpen ? styles.active : ''}`}
          onClick={() => togglePlugins()}
          title={t('commands.view.plugins')}
          aria-label={t('commands.view.plugins')}
          aria-pressed={pluginsOpen}
        >
          <Puzzle size={16} aria-hidden />
        </button>
        <button
          type="button"
          className={`${styles.tbBtn} ${syncOpen ? styles.active : ''}`}
          onClick={() => toggleSync()}
          title={t('commands.view.sync')}
          aria-label={t('commands.view.sync')}
          aria-pressed={syncOpen}
        >
          <GitBranch size={16} aria-hidden />
        </button>
        <button
          type="button"
          className={`${styles.tbBtn} ${goalsOpen ? styles.active : ''}`}
          onClick={() => toggleGoals()}
          title={t('commands.view.goals')}
          aria-label={t('commands.view.goals')}
          aria-pressed={goalsOpen}
        >
          <Target size={16} aria-hidden />
        </button>
        <button
          type="button"
          className={`${styles.tbBtn} ${digestOpen ? styles.active : ''}`}
          onClick={() => toggleDigest()}
          title={t('commands.view.digest')}
          aria-label={t('commands.view.digest')}
          aria-pressed={digestOpen}
        >
          <Newspaper size={16} aria-hidden />
        </button>

        <span className={styles.divider} />

        <button
          type="button"
          className={styles.tbBtn}
          onClick={() => toggleTheme()}
          title={t('commands.theme.toggle')}
          aria-label={t('commands.theme.toggle')}
        >
          {theme === 'dark' ? <Sun size={16} aria-hidden /> : <Moon size={16} aria-hidden />}
        </button>
        <button
          type="button"
          className={`${styles.tbBtn} ${tweaksOpen ? styles.active : ''}`}
          onClick={() => toggleTweaks()}
          title={t('commands.view.settings')}
          aria-label={t('commands.view.settings')}
          aria-pressed={tweaksOpen}
        >
          <SlidersHorizontal size={16} aria-hidden />
        </button>
        <button
          type="button"
          className={styles.tbLang}
          onClick={() => changeLocale(lang === 'ru' ? 'en' : 'ru')}
          title="Язык / Language"
          aria-label="Язык / Language"
        >
          <span className={lang === 'ru' ? styles.on : ''}>RU</span>
          <span className={styles.sep}>·</span>
          <span className={lang === 'en' ? styles.on : ''}>EN</span>
        </button>
        <button
          type="button"
          className={styles.tbBtn}
          onClick={() => void openVaultFlow()}
          title={t('app.openVault')}
          aria-label={t('app.openVault')}
        >
          <FolderOpen size={16} aria-hidden />
        </button>
      </div>
    </div>
  );
}
