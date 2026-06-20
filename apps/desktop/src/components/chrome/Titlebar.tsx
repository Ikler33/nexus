import { useEffect, useRef, useState } from 'react';
import {
  BookOpen,
  ChevronDown,
  HardDrive,
  Moon,
  Newspaper,
  Palette,
  PanelRight,
  Scale,
  Search,
  Sparkles,
  Sun,
  Target,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { changeLocale } from '../../i18n/setup';
import { useThemeStore, type Theme } from '../../stores/theme';
import { useUIStore } from '../../stores/ui';
import { BrandMark } from './BrandMark';
import styles from './Titlebar.module.css';

/**
 * Иконка кнопки-цикла темы. Базовые 4 темы — характерные иконки (sun → moon →
 * sparkles → drive); прочие 9 (paper/mocha/nord/… — QASR-0) дают общий
 * Palette-знак (визуальный язык кнопки доработает QASR-shell). Fallback нужен,
 * чтобы цикл по 13 темам никогда не оставлял кнопку пустой.
 */
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
    default:
      return <Palette size={16} aria-hidden />;
  }
}

/**
 * Верхний titlebar по макету `app.jsx` (DP-13): бренд «лого + Nexus» (клик → Home),
 * центральная поисковая пилюля (⌘K), справа ТОЛЬКО AI-инсайты (sparkles▾) | divider |
 * режим чтения | RU/EN | тема | panel-right (AI-панель). Граф/новости/sync/настройки
 * переехали в вертикальный ActivityBar; плагины и «Открыть vault» — команды палитры.
 */
export function Titlebar() {
  const { t, i18n } = useTranslation();
  const openPalette = useUIStore((s) => s.openPalette);
  const openHome = useUIStore((s) => s.openHome);
  const chatOpen = useUIStore((s) => s.chatOpen);
  const toggleChat = useUIStore((s) => s.toggleChat);
  const toggleGoals = useUIStore((s) => s.toggleGoals);
  const toggleDigest = useUIStore((s) => s.toggleDigest);
  const toggleContradictions = useUIStore((s) => s.toggleContradictions);
  const reading = useUIStore((s) => s.reading);
  const toggleReading = useUIStore((s) => s.toggleReading);
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
      <button
        type="button"
        className={styles.brand}
        onClick={() => openHome()}
        title={t('commands.view.home')}
        aria-label={t('commands.view.home')}
      >
        <BrandMark size={24} />
        <span className={styles.appName}>{t('app.name')}</span>
      </button>

      <span className={styles.spacer} />
      <button type="button" className={styles.search} onClick={() => openPalette()}>
        <Search size={14} aria-hidden />
        <span>{t('chrome.search')}</span>
        <kbd className={styles.kbd}>⌘K</kbd>
      </button>
      <span className={styles.spacer} />

      <div className={styles.group}>
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
          className={`${styles.tbBtn} ${chatOpen ? styles.active : ''}`}
          onClick={() => toggleChat()}
          title={t('chrome.aiPanel')}
          aria-label={t('chrome.aiPanel')}
          aria-pressed={chatOpen}
        >
          <PanelRight size={16} aria-hidden />
        </button>
      </div>
    </div>
  );
}
