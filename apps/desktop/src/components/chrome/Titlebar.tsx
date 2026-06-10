import { useEffect, useRef, useState } from 'react';
import {
  BookOpen,
  ChevronDown,
  FolderOpen,
  GitBranch,
  HardDrive,
  MessageSquare,
  Moon,
  Newspaper,
  Rss,
  Scale,
  Puzzle,
  Search,
  Share2,
  SlidersHorizontal,
  Sparkles,
  Sun,
  Target,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { changeLocale } from '../../i18n/setup';
import { openVaultFlow } from '../../lib/commands-core';
import { useThemeStore, type Theme } from '../../stores/theme';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { BrandMark } from './BrandMark';
import styles from './Titlebar.module.css';

/** Иконка темы (DP-4, цикл макета): sun → moon → sparkles → drive. */
function themeIcon(theme: Theme) {
  switch (theme) {
    case 'light':
      return <Sun size={16} aria-hidden />;
    case 'dark':
      return <Moon size={16} aria-hidden />;
    case 'midnight':
      return <Sparkles size={16} aria-hidden />;
    case 'platinum':
      return <HardDrive size={16} aria-hidden />;
  }
}

/**
 * Верхний titlebar дизайн-системы (Liquid-Glass): бренд-марк + имя vault, центральная
 * поисковая пилюля (⌘K) и правая группа инструментов. DP-4: AI-инсайты (Дайджест / Цели /
 * Противоречия) консолидированы в sparkles-меню (как в макете), добавлен тоггл режима
 * чтения, тема циклится по 4 темам.
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
  const toggleGoals = useUIStore((s) => s.toggleGoals);
  const toggleDigest = useUIStore((s) => s.toggleDigest);
  const toggleContradictions = useUIStore((s) => s.toggleContradictions);
  const newsOpen = useUIStore((s) => s.newsOpen);
  const toggleNews = useUIStore((s) => s.toggleNews);
  const reading = useUIStore((s) => s.reading);
  const toggleReading = useUIStore((s) => s.toggleReading);
  const tweaksOpen = useUIStore((s) => s.tweaksOpen);
  const toggleTweaks = useUIStore((s) => s.toggleTweaks);
  const theme = useThemeStore((s) => s.theme);
  const toggleTheme = useThemeStore((s) => s.toggle);
  const lang = i18n.language === 'ru' ? 'ru' : 'en';
  const [aiMenu, setAiMenu] = useState(false);
  const aiRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!aiMenu) return;
    const onDown = (e: MouseEvent) => {
      if (aiRef.current && !aiRef.current.contains(e.target as Node)) setAiMenu(false);
    };
    window.addEventListener('mousedown', onDown);
    return () => window.removeEventListener('mousedown', onDown);
  }, [aiMenu]);

  const aiItem = (icon: React.ReactNode, label: string, run: () => void) => (
    <button
      type="button"
      className={styles.aiMenuItem}
      role="menuitem"
      onClick={() => {
        setAiMenu(false);
        run();
      }}
    >
      {icon}
      {label}
    </button>
  );

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

        {/* AI-инсайты: Дайджест / Цели / Противоречия — в выпадающем меню (DP-4, макет). */}
        <div className={styles.aiWrap} ref={aiRef}>
          <button
            type="button"
            className={`${styles.tbBtn} ${aiMenu ? styles.active : ''}`}
            onClick={() => setAiMenu((v) => !v)}
            title={t('chrome.aiMenu')}
            aria-label={t('chrome.aiMenu')}
            aria-expanded={aiMenu}
            aria-haspopup="menu"
          >
            <Sparkles size={16} aria-hidden />
            <ChevronDown size={11} aria-hidden />
          </button>
          {aiMenu && (
            <div className={styles.aiMenu} role="menu" aria-label={t('chrome.aiMenu')}>
              <div className={styles.aiMenuHead}>{t('chrome.aiMenu')}</div>
              {aiItem(
                <Newspaper size={15} aria-hidden />,
                t('commands.view.digest'),
                toggleDigest,
              )}
              {aiItem(<Target size={15} aria-hidden />, t('commands.view.goals'), toggleGoals)}
              {aiItem(
                <Scale size={15} aria-hidden />,
                t('commands.view.contradictions'),
                toggleContradictions,
              )}
            </div>
          )}
        </div>

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
          className={`${styles.tbBtn} ${newsOpen ? styles.active : ''}`}
          onClick={() => toggleNews()}
          title={t('commands.view.news')}
          aria-label={t('commands.view.news')}
          aria-pressed={newsOpen}
        >
          <Rss size={16} aria-hidden />
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

        <span className={styles.divider} />

        <button
          type="button"
          className={`${styles.tbBtn} ${reading ? styles.active : ''}`}
          onClick={() => toggleReading()}
          title={t('commands.view.reading')}
          aria-label={t('commands.view.reading')}
          aria-pressed={reading}
        >
          <BookOpen size={16} aria-hidden />
        </button>
        <button
          type="button"
          className={styles.tbBtn}
          onClick={() => toggleTheme()}
          title={t('commands.theme.toggle')}
          aria-label={t('commands.theme.toggle')}
        >
          <span key={theme} className={styles.themeIco}>
            {themeIcon(theme)}
          </span>
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
