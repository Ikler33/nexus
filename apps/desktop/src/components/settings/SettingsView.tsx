import { useEffect, useReducer, useRef, useState, type ComponentType } from 'react';
import {
  AlertCircle,
  Check,
  Cpu,
  Database,
  Download,
  Globe,
  Info,
  Keyboard,
  Loader2,
  Palette,
  Pencil,
  RotateCcw,
  Settings as SettingsIcon,
  Upload,
  X,
} from 'lucide-react';
import { OrbitIcon } from '../chrome/BrandGlyphs';
import { useTranslation } from 'react-i18next';

import { BrandMark } from '../chrome/BrandMark';
import { useFocusTrap } from '../../hooks/useFocusTrap';
import { changeLocale } from '../../i18n/setup';
import { commands, eventToCombo, formatCombo, spellCombo } from '../../lib/commands';
import { tauriApi } from '../../lib/tauri-api';
import type {
  AgentFlagsDto,
  BackupImportReport,
  EgressState,
  WebSearchConfig,
} from '../../lib/tauri-api';
import { useAiFeaturesStore } from '../../stores/aiFeatures';
import { useEpisodeStore } from '../../stores/episode';
import { usePrefsStore } from '../../stores/prefs';
import { ACCENTS, THEMES, useThemeStore } from '../../stores/theme';
import type { Accent, Theme } from '../../stores/theme';
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

/**
 * Превью-цвета свотча темы (bg/text/accent) — зеркало data-theme в styles.css.
 * Карточка темы рисует мини-превью реальной палитры без переключения темы документа.
 */
const THEME_PREVIEW: Record<Theme, { bg: string; text: string; accent: string }> = {
  light: { bg: '#ECE6DA', text: '#211D17', accent: '#C45B33' },
  dark: { bg: '#16140F', text: '#EDE7DA', accent: '#D86E44' },
  midnight: {
    bg: 'oklch(0.165 0.014 264)',
    text: 'oklch(0.925 0.010 264)',
    accent: 'oklch(0.84 0.072 88)',
  },
  platinum: {
    bg: 'oklch(0.205 0.008 250)',
    text: 'oklch(0.945 0.005 250)',
    accent: 'oklch(0.82 0.020 248)',
  },
  paper: {
    bg: 'oklch(0.977 0.0035 75)',
    text: 'oklch(0.225 0.006 75)',
    accent: 'oklch(0.255 0.007 75)',
  },
  mocha: { bg: '#1e1e2e', text: '#cdd6f4', accent: '#cba6f7' },
  nord: { bg: '#2e3440', text: '#eceff4', accent: '#88c0d0' },
  tokyo: { bg: '#1a1b26', text: '#c0caf5', accent: '#7aa2f7' },
  rose: { bg: '#191724', text: '#e0def4', accent: '#ebbcba' },
  sepia: { bg: '#f4ecd8', text: '#433422', accent: '#8a5a2b' },
  contrast: { bg: '#000000', text: '#ffffff', accent: '#4cc2ff' },
  bronze: { bg: '#13120e', text: '#ece4d2', accent: '#c9a35e' },
  marble: { bg: '#ece4d2', text: '#2a2418', accent: '#9a5a28' },
};

const SECTIONS: { id: SettingsSection; icon: typeof Palette; key: string }[] = [
  { id: 'general', icon: Globe, key: 'settings.general' },
  { id: 'editor', icon: Pencil, key: 'settings.editor' },
  { id: 'appearance', icon: Palette, key: 'settings.appearance' },
  { id: 'ai', icon: Cpu, key: 'settings.ai' },
  { id: 'data', icon: Database, key: 'settings.data' },
  { id: 'hotkeys', icon: Keyboard, key: 'settings.hotkeys' },
  { id: 'about', icon: Info, key: 'settings.about' },
];

/**
 * Раздел настроек (кросс-план #11, по образцу Obsidian): модалка с левым навом секций + контент-панель.
 * Секции: «Основное» (язык), «Редактор» (читаемая ширина), «Оформление» (тема/акцент/плотность),
 * «AI / Модели», «Горячие клавиши» (переназначение хоткеев), «О программе». Состояние
 * открытия/активной секции — в ui-сторе.
 */
export function SettingsView() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeTweaks);
  const trapRef = useFocusTrap<HTMLDivElement>(close); // a11y: Esc/Tab-цикл внутри модалки (audit B10)
  const section = useUIStore((s) => s.settingsSection);
  const setSection = useUIStore((s) => s.setSettingsSection);

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        ref={trapRef}
        tabIndex={-1}
        className={styles.modal}
        role="dialog"
        aria-modal="true"
        aria-label={t('settings.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <span className={styles.headIcon} aria-hidden>
            <SettingsIcon size={17} />
          </span>
          <div className={styles.headTitle}>{t('settings.title')}</div>
          <button type="button" className={styles.close} onClick={close} aria-label={t('git.close')}>
            <X size={16} aria-hidden />
          </button>
        </header>

        <nav className={styles.nav} aria-label={t('settings.title')}>
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
          {section === 'general' && <GeneralSection />}
          {section === 'editor' && <EditorSection />}
          {section === 'appearance' && <AppearanceSection />}
          {section === 'ai' && <AiSection />}
          {section === 'data' && <DataSection />}
          {section === 'hotkeys' && <HotkeysSection />}
          {section === 'about' && <AboutSection />}
        </div>
      </div>
    </div>
  );
}

/** Заголовок секции + подзаголовок (макет Qasr: .set-sec-title над .set-sec-sub). */
function SectionHeader({ title, sub, nested }: { title: string; sub?: string; nested?: boolean }) {
  return (
    <header className={`${styles.secHead} ${nested ? styles.secHeadNested : ''}`}>
      <h2 className={styles.secTitle}>{title}</h2>
      {sub && <p className={styles.secSub}>{sub}</p>}
    </header>
  );
}

function GeneralSection() {
  const { t, i18n } = useTranslation();
  const lang = i18n.language === 'ru' ? 'ru' : 'en';
  const userName = usePrefsStore((s) => s.userName);
  const setUserName = usePrefsStore((s) => s.setUserName);
  const paletteStyle = usePrefsStore((s) => s.paletteStyle);
  const setPaletteStyle = usePrefsStore((s) => s.setPaletteStyle);
  const aiLayout = usePrefsStore((s) => s.aiLayout);
  const setAiLayout = usePrefsStore((s) => s.setAiLayout);
  const ragSources = usePrefsStore((s) => s.ragSources);
  const setRagSources = usePrefsStore((s) => s.setRagSources);
  return (
    <>
      <SectionHeader title={t('settings.general')} />
      <section className={styles.group}>
        <span className={styles.label}>{t('settings.gen.language')}</span>
        <div className={styles.seg}>
          <button
            type="button"
            className={`${styles.segBtn} ${lang === 'ru' ? styles.on : ''}`}
            onClick={() => changeLocale('ru')}
            aria-pressed={lang === 'ru'}
          >
            Русский
          </button>
          <button
            type="button"
            className={`${styles.segBtn} ${lang === 'en' ? styles.on : ''}`}
            onClick={() => changeLocale('en')}
            aria-pressed={lang === 'en'}
          >
            English
          </button>
        </div>
      </section>
      <section className={styles.group}>
        <label className={styles.field}>
          <span>{t('settings.gen.userName')}</span>
          <input
            value={userName}
            onChange={(e) => setUserName(e.target.value)}
            placeholder={t('settings.gen.userNamePlaceholder')}
          />
        </label>
        <p className={styles.hint}>{t('settings.gen.userNameHint')}</p>
      </section>
      <section className={styles.group}>
        <span className={styles.label}>{t('tweaks.paletteStyle')}</span>
        <div className={styles.seg}>
          {(['top', 'center', 'spotlight'] as const).map((p) => (
            <button
              key={p}
              type="button"
              className={`${styles.segBtn} ${paletteStyle === p ? styles.on : ''}`}
              onClick={() => setPaletteStyle(p)}
            >
              {t(`tweaks.palette.${p}`)}
            </button>
          ))}
        </div>
      </section>
      {/* DP-12 (макет tweaks): расположение AI-панели + стиль RAG-источников в чате. */}
      <section className={styles.group}>
        <span className={styles.label}>{t('tweaks.aiLayout')}</span>
        <div className={styles.seg}>
          {(['side', 'bottom', 'overlay'] as const).map((p) => (
            <button
              key={p}
              type="button"
              className={`${styles.segBtn} ${aiLayout === p ? styles.on : ''}`}
              onClick={() => setAiLayout(p)}
            >
              {t(`tweaks.aiLayoutOpts.${p}`)}
            </button>
          ))}
        </div>
      </section>
      <section className={styles.group}>
        <span className={styles.label}>{t('tweaks.ragSources')}</span>
        <div className={styles.seg}>
          {(['cards', 'chips', 'footnotes'] as const).map((p) => (
            <button
              key={p}
              type="button"
              className={`${styles.segBtn} ${ragSources === p ? styles.on : ''}`}
              onClick={() => setRagSources(p)}
            >
              {t(`tweaks.ragSourcesOpts.${p}`)}
            </button>
          ))}
        </div>
      </section>
    </>
  );
}

function EditorSection() {
  const { t } = useTranslation();
  const readable = usePrefsStore((s) => s.readableLineWidth);
  const setReadable = usePrefsStore((s) => s.setReadableLineWidth);
  return (
    <>
      <SectionHeader title={t('settings.editor')} />
      <section className={styles.group}>
        <div className={styles.rowText}>
          <span className={styles.label}>{t('settings.ed.readableWidth')}</span>
          <span className={styles.rowDesc}>{t('settings.ed.readableWidthDesc')}</span>
        </div>
        <div className={styles.seg}>
          <button
            type="button"
            className={`${styles.segBtn} ${!readable ? styles.on : ''}`}
            onClick={() => setReadable(false)}
            aria-pressed={!readable}
          >
            {t('settings.off')}
          </button>
          <button
            type="button"
            className={`${styles.segBtn} ${readable ? styles.on : ''}`}
            onClick={() => setReadable(true)}
            aria-pressed={readable}
          >
            {t('settings.on')}
          </button>
        </div>
      </section>
    </>
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
  const chrome = useThemeStore((s) => s.chrome);
  const setChrome = useThemeStore((s) => s.setChrome);
  const editorFont = useThemeStore((s) => s.editorFont);
  const setEditorFont = useThemeStore((s) => s.setEditorFont);

  return (
    <>
      <SectionHeader title={t('settings.appearance')} />
      <section className={`${styles.group} ${styles.groupStack}`}>
        <span className={styles.label}>{t('tweaks.theme')}</span>
        <div className={styles.themeGrid}>
          {THEMES.map((th) => {
            const p = THEME_PREVIEW[th];
            return (
              <button
                key={th}
                type="button"
                className={`${styles.themeCard} ${theme === th ? styles.themeCardOn : ''}`}
                onClick={() => setTheme(th)}
                aria-pressed={theme === th}
              >
                <span
                  className={styles.themeSwatch}
                  style={{ background: p.bg, color: p.text }}
                  aria-hidden
                >
                  <i className={styles.themeSwatchLine} style={{ background: p.text }} />
                  <b className={styles.themeSwatchDot} style={{ background: p.accent }} />
                </span>
                <span className={styles.themeLabel}>{t(`tweaks.themes.${th}`)}</span>
              </button>
            );
          })}
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
          <button
            type="button"
            className={`${styles.segBtn} ${density === 'auto' ? styles.on : ''}`}
            onClick={() => setDensity('auto')}
          >
            {t('tweaks.auto')}
          </button>
        </div>
      </section>
      <section className={styles.group}>
        <span className={styles.label}>{t('tweaks.chrome')}</span>
        <div className={styles.seg}>
          <button
            type="button"
            className={`${styles.segBtn} ${chrome === 'standard' ? styles.on : ''}`}
            onClick={() => setChrome('standard')}
          >
            {t('tweaks.chromeStandard')}
          </button>
          <button
            type="button"
            className={`${styles.segBtn} ${chrome === 'minimal' ? styles.on : ''}`}
            onClick={() => setChrome('minimal')}
          >
            {t('tweaks.chromeMinimal')}
          </button>
        </div>
      </section>
      <section className={styles.group}>
        <span className={styles.label}>{t('tweaks.editorFont')}</span>
        <div className={styles.seg}>
          {(['sans', 'serif', 'mono'] as const).map((f) => (
            <button
              key={f}
              type="button"
              className={`${styles.segBtn} ${editorFont === f ? styles.on : ''}`}
              onClick={() => setEditorFont(f)}
            >
              {t(`tweaks.font.${f}`)}
            </button>
          ))}
        </div>
      </section>
    </>
  );
}

/**
 * W-9 (#59): backup/restore «второго мозга» (факты/переписка/эпизоды/навыки). Экспорт → файл через
 * save-диалог; импорт → файл через open-диалог с дедупом, показываем отчёт. fs — в бэкенде.
 */
function DataSection() {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<'export' | 'import' | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [report, setReport] = useState<BackupImportReport | null>(null);

  const doExport = async () => {
    setBusy('export');
    setMsg(null);
    setErr(null);
    setReport(null);
    try {
      const path = await tauriApi.backup.exportToFile();
      if (path) setMsg(t('settings.dataSec.exportedTo', { path }));
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  const doImport = async () => {
    setBusy('import');
    setMsg(null);
    setErr(null);
    setReport(null);
    try {
      const r = await tauriApi.backup.importFromFile();
      if (r) setReport(r);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div>
      <SectionHeader title={t('settings.dataSec.title')} sub={t('settings.dataSec.sub')} />
      <section className={styles.group}>
        <div className={styles.dataActions}>
          <button
            type="button"
            className={styles.dataBtn}
            onClick={() => void doExport()}
            disabled={busy !== null}
          >
            <Download size={15} aria-hidden />
            {busy === 'export' ? t('settings.dataSec.exporting') : t('settings.dataSec.export')}
          </button>
          <button
            type="button"
            className={styles.dataBtn}
            onClick={() => void doImport()}
            disabled={busy !== null}
          >
            <Upload size={15} aria-hidden />
            {busy === 'import' ? t('settings.dataSec.importing') : t('settings.dataSec.import')}
          </button>
        </div>
        {msg && (
          <p className={styles.dataOk} role="status">
            {msg}
          </p>
        )}
        {err && (
          <p className={styles.dataErr} role="alert">
            {err}
          </p>
        )}
        {report && (
          <div className={styles.dataReport} role="status">
            <div className={styles.dataReportHead}>{t('settings.dataSec.imported')}</div>
            <ul className={styles.dataReportList}>
              <li>
                {t('settings.dataSec.rFacts', {
                  added: report.factsAdded,
                  skipped: report.factsSkipped,
                })}
              </li>
              <li>
                {t('settings.dataSec.rSessions', {
                  added: report.sessionsAdded,
                  reused: report.sessionsReused,
                })}
              </li>
              <li>
                {t('settings.dataSec.rMessages', {
                  added: report.messagesAdded,
                  skipped: report.messagesSkipped,
                })}
              </li>
              <li>
                {t('settings.dataSec.rEpisodes', {
                  added: report.episodesAdded,
                  skipped: report.episodesSkipped,
                })}
              </li>
              <li>
                {t('settings.dataSec.rSkills', {
                  added: report.skillsAdded,
                  skipped: report.skillsSkipped,
                })}
              </li>
            </ul>
            {(report.messagesOrphaned > 0 || report.episodesOrphaned > 0) && (
              <p className={styles.dataWarn}>
                {t('settings.dataSec.orphaned', {
                  count: report.messagesOrphaned + report.episodesOrphaned,
                })}
              </p>
            )}
            {report.schemaVersionMismatch && (
              <p className={styles.dataWarn}>{t('settings.dataSec.schemaOld')}</p>
            )}
          </div>
        )}
      </section>
    </div>
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
    <div className={styles.about}>
      <BrandMark size={56} />
      <div className={styles.aboutName}>{t('app.name')}</div>
      <div className={styles.aboutVer}>
        {t('settings.version')} {version}
      </div>
      <div className={styles.aboutMeta}>
        {t('settings.vault')}: {vaultRoot ?? t('settings.noVault')}
      </div>
    </div>
  );
}

type TestState = { status: 'idle' | 'testing' | 'ok' | 'fail'; msg?: string };

/**
 * Секция «AI / Модели» (кросс-план #11, слайс 2): форма эндпоинтов chat/embedding с проверкой связи
 * и сохранением в `.nexus/local.json` через нативные команды. Chat применяется немедленно; смена
 * embedding требует перезапуска (на нём висит индексатор) — об этом сообщаем после сохранения.
 */
function AiSection() {
  const { t } = useTranslation();
  const aiRerank = usePrefsStore((s) => s.aiRerank);
  const setAiRerank = usePrefsStore((s) => s.setAiRerank);
  const aiChatMemory = usePrefsStore((s) => s.aiChatMemory);
  const setAiChatMemory = usePrefsStore((s) => s.setAiChatMemory);
  const aiAgentMemory = usePrefsStore((s) => s.aiAgentMemory);
  const setAiAgentMemory = usePrefsStore((s) => s.setAiAgentMemory);
  const aiMemoryConsolidation = usePrefsStore((s) => s.aiMemoryConsolidation);
  const setAiMemoryConsolidation = usePrefsStore((s) => s.setAiMemoryConsolidation);
  const aiMemoryConsolidationMode = usePrefsStore((s) => s.aiMemoryConsolidationMode);
  const setAiMemoryConsolidationMode = usePrefsStore((s) => s.setAiMemoryConsolidationMode);
  const aiExplainRelations = usePrefsStore((s) => s.aiExplainRelations);
  const setAiExplainRelations = usePrefsStore((s) => s.setAiExplainRelations);
  const aiEpisodicMemory = usePrefsStore((s) => s.aiEpisodicMemory);
  const setAiEpisodicMemory = usePrefsStore((s) => s.setAiEpisodicMemory);
  // Фоновые ИИ-фичи Home (persisted в БД vault, дефолт OFF) — гейтятся owner-тогглами.
  const insightsEnabled = useAiFeaturesStore((s) => s.insights);
  const setInsightsEnabled = useAiFeaturesStore((s) => s.setInsights);
  const contradictionsEnabled = useAiFeaturesStore((s) => s.contradictions);
  const setContradictionsEnabled = useAiFeaturesStore((s) => s.setContradictions);
  const aiChatDeep = usePrefsStore((s) => s.aiChatDeep);
  const setAiChatDeep = usePrefsStore((s) => s.setAiChatDeep);
  const openMemory = useUIStore((s) => s.openMemory);
  const openEpisodes = useUIStore((s) => s.openEpisodes);
  const [chatUrl, setChatUrl] = useState('');
  const [chatModel, setChatModel] = useState('');
  const [embUrl, setEmbUrl] = useState('');
  const [embModel, setEmbModel] = useState('');
  const [fastUrl, setFastUrl] = useState('');
  const [fastModel, setFastModel] = useState('');
  const [fastTest, setFastTest] = useState<TestState>({ status: 'idle' });
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [restart, setRestart] = useState(false);
  const [chatTest, setChatTest] = useState<TestState>({ status: 'idle' });
  const [embTest, setEmbTest] = useState<TestState>({ status: 'idle' });

  useEffect(() => {
    let alive = true;
    void tauriApi.settings.getAiConfig().then((cfg) => {
      if (!alive) return;
      setChatUrl(cfg.chat?.url ?? '');
      setChatModel(cfg.chat?.model ?? '');
      setEmbUrl(cfg.embedding?.url ?? '');
      setEmbModel(cfg.embedding?.model ?? '');
      setFastUrl(cfg.fast?.url ?? '');
      setFastModel(cfg.fast?.model ?? '');
    });
    return () => {
      alive = false;
    };
  }, []);

  const runTest = async (url: string, set: (s: TestState) => void) => {
    const u = url.trim();
    if (!u) {
      set({ status: 'fail', msg: t('settings.aiSec.urlRequired') });
      return;
    }
    set({ status: 'testing' });
    try {
      await tauriApi.settings.testConnection(u);
      set({ status: 'ok' });
    } catch (e) {
      set({ status: 'fail', msg: String(e) });
    }
  };

  const save = async () => {
    setSaving(true);
    setSaved(false);
    const chat = chatUrl.trim() ? { url: chatUrl.trim(), model: chatModel.trim() || null } : null;
    const embedding = embUrl.trim()
      ? { url: embUrl.trim(), model: embModel.trim() || null }
      : null;
    const fast = fastUrl.trim() ? { url: fastUrl.trim(), model: fastModel.trim() || null } : null;
    try {
      const res = await tauriApi.settings.setAiConfig(chat, embedding, fast);
      setRestart(res.embeddingChanged);
      setSaved(true);
    } catch (e) {
      setChatTest({ status: 'fail', msg: String(e) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <SectionHeader title={t('settings.ai')} sub={t('settings.aiSec.intro')} />

      <Endpoint
        icon={OrbitIcon}
        title={t('settings.aiSec.chatTitle')}
        desc={t('settings.aiSec.chatDesc')}
        url={chatUrl}
        model={chatModel}
        onUrl={setChatUrl}
        onModel={setChatModel}
        test={chatTest}
        onTest={() => void runTest(chatUrl, setChatTest)}
      />
      <Endpoint
        icon={Cpu}
        title={t('settings.aiSec.embTitle')}
        desc={t('settings.aiSec.embDesc')}
        url={embUrl}
        model={embModel}
        onUrl={setEmbUrl}
        onModel={setEmbModel}
        test={embTest}
        onTest={() => void runTest(embUrl, setEmbTest)}
      />
      <Endpoint
        icon={Cpu}
        title={t('settings.aiSec.fastTitle')}
        desc={t('settings.aiSec.fastDesc')}
        url={fastUrl}
        model={fastModel}
        onUrl={setFastUrl}
        onModel={setFastModel}
        test={fastTest}
        onTest={() => void runTest(fastUrl, setFastTest)}
      />

      <div className={styles.saveBar}>
        <button type="button" className={styles.primaryBtn} onClick={() => void save()} disabled={saving}>
          {saving ? t('settings.aiSec.saving') : t('settings.aiSec.save')}
        </button>
        {saved && !restart && <span className={styles.okText}>{t('settings.aiSec.saved')}</span>}
        {saved && restart && <span className={styles.warnText}>{t('settings.aiSec.restart')}</span>}
      </div>

      {/* Reasoning-режим чата (замер 2026-06-18): «Быстрый» (без CoT) vs «Глубокий» (с CoT gemma).
          ВЫКЛ по умолчанию = Быстрый — на RAG-по-базе reasoning давал +30–40с без выигрыша качества;
          «Глубокий» оставлен тогглом на редкие сложные выводы. Per-call флаг `deep` в chat_rag. */}
      <EgressRow
        label={t('settings.aiSec.chatDeep')}
        desc={t('settings.aiSec.chatDeepDesc')}
        value={aiChatDeep}
        onChange={setAiChatDeep}
      />

      {/* LLM-реранжирование источников (search::rerank, eval: nDCG .883→1.0 / MRR .848→1.0):
          переупорядочивает топ-24 кандидатов мелкой моделью перед выбором k. Цена ~1–3 с. */}
      <EgressRow
        label={t('settings.aiSec.rerank')}
        desc={t('settings.aiSec.rerankDesc')}
        value={aiRerank}
        onChange={setAiRerank}
      />

      {/* Память переписки (N4b): подмешивать релевантные фрагменты прошлых диалогов как фон.
          Отдельный канал (chat_vectors) — не влияет на поиск по заметкам. ВКЛ по умолчанию. */}
      <EgressRow
        label={t('settings.aiSec.chatMemory')}
        desc={t('settings.aiSec.chatMemoryDesc')}
        value={aiChatMemory}
        onChange={setAiChatMemory}
      />

      {/* MEM (память агента): подмешивать сохранённые ЯВНЫЕ ФАКТЫ о пользователе/проектах в ответы.
          ВЫКЛ по умолчанию (D5: приватность-first). Управление фактами — кнопка «Память ИИ». */}
      <EgressRow
        label={t('settings.aiSec.agentMemory')}
        desc={t('settings.aiSec.agentMemoryDesc')}
        value={aiAgentMemory}
        onChange={setAiAgentMemory}
      />
      {/* MEM-8b (owner-gated): консолидация — при подтверждении факта ИИ предлагает объединить/заменить
          близкий существующий (режим «Предлагать»: каждое слияние/замещение через клик, обратимо).
          Имеет смысл только при включённой памяти агента → дизейблим без неё. */}
      <EgressRow
        label={t('settings.aiSec.memoryConsolidation')}
        desc={t('settings.aiSec.memoryConsolidationDesc')}
        value={aiMemoryConsolidation && aiAgentMemory}
        onChange={setAiMemoryConsolidation}
        disabled={!aiAgentMemory}
      />
      {/* MEM-8c-b: режим консолидации — «Предлагать» (через чип) ↔ «Авто» (применять молча, с undo).
          Виден только когда консолидация реально включена. */}
      {aiMemoryConsolidation && aiAgentMemory && (
        <section className={styles.group}>
          <div className={styles.rowText}>
            <span className={styles.label}>{t('settings.aiSec.consolidationMode')}</span>
            <span className={styles.rowDesc}>{t('settings.aiSec.consolidationModeDesc')}</span>
          </div>
          <div className={styles.seg}>
            {(['propose', 'auto'] as const).map((m) => (
              <button
                key={m}
                type="button"
                className={`${styles.segBtn} ${aiMemoryConsolidationMode === m ? styles.on : ''}`}
                onClick={() => setAiMemoryConsolidationMode(m)}
                aria-pressed={aiMemoryConsolidationMode === m}
              >
                {t(`settings.aiSec.consolidationModeOpts.${m}`)}
              </button>
            ))}
          </div>
        </section>
      )}
      {/* EP-3: эпизодическая память — подмешивать саммари прошлых сессий в ответы. ВЫКЛ по умолчанию.
          Тоггл пишет И фронт-pref (per-call флаг чата), И persisted-настройку бэка (фоновая генерация +
          kick при включении — контракт MAJOR-2). Управление эпизодами — кнопка «Эпизоды…». */}
      <EgressRow
        label={t('settings.aiSec.episodicMemory')}
        desc={t('settings.aiSec.episodicMemoryDesc')}
        value={aiEpisodicMemory}
        onChange={(on) => {
          setAiEpisodicMemory(on);
          void useEpisodeStore.getState().setEnabled(on);
        }}
      />
      <div className={styles.saveBar}>
        <button type="button" className={styles.ghostBtn} onClick={openMemory}>
          {t('settings.aiSec.manageMemory')}
        </button>
        <button type="button" className={styles.ghostBtn} onClick={openEpisodes}>
          {t('settings.aiSec.manageEpisodes')}
        </button>
      </div>

      {/* Фоновые ИИ-фичи Home, гейтируемые владельцем (real-test 2026-06-18). Дефолт OFF (opt-in). При
          включении бэкенд ставит kick-генерацию (иначе фича «мертва» до перезапуска vault — урок EP-1).
          Источник истины — БД vault (стор грузится от бэка на открытии). */}
      <EgressRow
        label={t('settings.aiSec.insights')}
        desc={t('settings.aiSec.insightsDesc')}
        value={insightsEnabled}
        onChange={(on) => void setInsightsEnabled(on)}
      />
      <EgressRow
        label={t('settings.aiSec.contradictions')}
        desc={t('settings.aiSec.contradictionsDesc')}
        value={contradictionsEnabled}
        onChange={(on) => void setContradictionsEnabled(on)}
      />

      {/* AIP-10: LLM-«причина связи» в «Связях»/«Похожих» (лениво, кэш). Без утилитарной модели —
          фолбэк на сниппет. */}
      <EgressRow
        label={t('settings.aiSec.explainRelations')}
        desc={t('settings.aiSec.explainRelationsDesc')}
        value={aiExplainRelations}
        onChange={setAiExplainRelations}
      />

      <EgressBlock />
      <WebSearchBlock />
      <HeadlessAgentBlock />
    </>
  );
}

/**
 * Настройки web-агента (W-3): URL SearXNG + тоггл. Сохранение непустого URL с включённым тогглом =
 * явный consent (W2) — показываем warning-баннер «запросы уйдут на этот хост». URL пуст → web-режим
 * чата работать не будет.
 */
function WebSearchBlock() {
  const { t } = useTranslation();
  const [cfg, setCfg] = useState<WebSearchConfig | null>(null);
  const [url, setUrl] = useState('');
  const [saved, setSaved] = useState(false);
  const [err, setErr] = useState<string | null>(null); // B16: сбой setConfig больше не глотаем

  useEffect(() => {
    let alive = true;
    void tauriApi.websearch
      .getConfig()
      .then((c) => {
        if (alive) {
          setCfg(c);
          setUrl(c.url);
        }
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  if (!cfg) return null;
  const persist = (next: WebSearchConfig) => {
    setSaved(false);
    setErr(null);
    void tauriApi.websearch
      .setConfig(next)
      .then((applied) => {
        setCfg(applied);
        setUrl(applied.url);
        setSaved(true);
      })
      .catch((e: unknown) => setErr(String(e))); // не молчим: пользователь думал, что сохранил (B16)
  };

  return (
    <>
      <SectionHeader title={t('settings.web.title')} sub={t('settings.web.intro')} nested />
      <section className={styles.group}>
        <label className={`${styles.field} ${styles.fieldWide}`}>
          <span>{t('settings.web.url')}</span>
          <input
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            onBlur={() => url !== cfg.url && persist({ ...cfg, url: url.trim() })}
            placeholder="https://searx.example.com"
            spellCheck={false}
          />
        </label>
        <p className={styles.hint}>{t('settings.web.urlHint')}</p>
      </section>
      {/* Consent-предупреждение: при активной фиче запросы реально уйдут на указанный хост. */}
      {url.trim() && cfg.enabled && (
        <p className={styles.warnText}>{t('settings.web.consentWarn', { host: hostOf(url) })}</p>
      )}
      <EgressRow
        label={t('settings.web.enable')}
        desc={t('settings.web.enableDesc')}
        value={cfg.enabled}
        onChange={(v) => persist({ ...cfg, url: url.trim(), enabled: v })}
      />
      {saved && <span className={styles.okText}>{t('settings.web.saved')}</span>}
      {err && <p className={styles.warnText}>{t('settings.web.saveError', { msg: err })}</p>}
    </>
  );
}

/**
 * Настройки АВТОНОМНОГО (headless) агента — Hermes-6/SYNC follow-up. ⚠️ Эти тогглы конфигурируют
 * ИСКЛЮЧИТЕЛЬНО серверный агент (`nexus-agentd` через коннектор): они персистятся в `.nexus/local.json`
 * и читаются им при старте. Десктопный ИИ-чат/панель агента ИМИ НЕ управляются (десктоп берёт автономию
 * прогона per-run в UI, а web — из отдельного `websearch.json`). Все по умолчанию OFF/confirm; опасные
 * включения дают consent-предупреждение (зеркало WebSearchBlock). sandbox/shell — Linux-only: на не-Linux
 * структурно инертны → тогглы disabled. shell зависит от sandbox (exec всегда Confirm, никогда Auto).
 */
function HeadlessAgentBlock() {
  const { t } = useTranslation();
  const [flags, setFlags] = useState<AgentFlagsDto | null>(null);
  const [shellSupported, setShellSupported] = useState(false);
  const [saved, setSaved] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  // Последний КОММИТНУТЫЙ набор — мерж patch'ей идёт от него, а не от замыкания рендера (иначе два
  // быстрых тоггла разных контролов до ре-рендера затирали бы друг друга стейлом). seq отбрасывает
  // устаревшие ответы бэка (out-of-order/flicker не клобберят свежий оптимистичный стейт).
  const flagsRef = useRef<AgentFlagsDto | null>(null);
  flagsRef.current = flags;
  const seqRef = useRef(0);

  useEffect(() => {
    let alive = true;
    void tauriApi.settings
      .getAiConfig()
      .then((c) => {
        if (!alive) return;
        setFlags({
          agentAutonomy: c.agentAutonomy,
          agentActuatorEnabled: c.agentActuatorEnabled,
          sandboxEnabled: c.sandboxEnabled,
          shellEnable: c.shellEnable,
          webAllowPublicFetch: c.webAllowPublicFetch,
        });
        setShellSupported(c.shellSupported);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  if (!flags) return null;
  /** Применяет частичный patch к ПОСЛЕДНЕМУ набору и персистит. Когерентность shell↔sandbox держим и
   * на фронте (зеркало бэка): shell не может быть true без песочницы. */
  const persist = (patch: Partial<AgentFlagsDto>) => {
    const base = flagsRef.current as AgentFlagsDto;
    const next: AgentFlagsDto = { ...base, ...patch };
    next.shellEnable = next.shellEnable && next.sandboxEnabled;
    setSaved(false);
    setErr(null);
    setFlags(next); // оптимистично: тоггл откликается сразу
    flagsRef.current = next;
    const seq = ++seqRef.current;
    void tauriApi.settings
      .setAgentFlags(next)
      .then((applied) => {
        if (seq !== seqRef.current) return; // устаревший ответ — пришёл новее
        setFlags(applied);
        flagsRef.current = applied;
        setSaved(true);
      })
      .catch((e: unknown) => {
        if (seq === seqRef.current) setErr(String(e));
      });
  };

  const autonomy = flags.agentAutonomy === 'auto' ? 'auto' : 'confirm';
  // host-exec доступен лишь на Linux И при включённой песочнице (иначе exec → HardBlocked у агентд).
  const shellAvailable = shellSupported && flags.sandboxEnabled;

  return (
    <>
      <SectionHeader title={t('settings.agent.title')} sub={t('settings.agent.intro')} nested />

      {/* Автономия headless-коннектора (confirm|auto). Десктопные прогоны выбирают её per-run отдельно. */}
      <section className={styles.group}>
        <div className={styles.rowText}>
          <span className={styles.label}>{t('settings.agent.autonomy')}</span>
          <span className={styles.rowDesc}>{t('settings.agent.autonomyDesc')}</span>
        </div>
        <div className={styles.seg}>
          {(['confirm', 'auto'] as const).map((m) => (
            <button
              key={m}
              type="button"
              className={`${styles.segBtn} ${autonomy === m ? styles.on : ''}`}
              onClick={() => persist({ agentAutonomy: m })}
              aria-pressed={autonomy === m}
            >
              {t(`settings.agent.autonomyOpts.${m}`)}
            </button>
          ))}
        </div>
      </section>
      {autonomy === 'auto' && <p className={styles.warnText}>{t('settings.agent.autonomyWarn')}</p>}

      {/* AGENT-0.6: мастер-свитч реальных действий агента в vault (создать/править заметку через
          approval-гейт). OFF (дефолт) → инструменты-заглушки. Читает и десктоп-агент, и agentd. */}
      <EgressRow
        label={t('settings.agent.actuator')}
        desc={t('settings.agent.actuatorDesc')}
        value={flags.agentActuatorEnabled}
        onChange={(v) => persist({ agentActuatorEnabled: v })}
      />
      {flags.agentActuatorEnabled && (
        <p className={styles.warnText}>{t('settings.agent.actuatorWarn')}</p>
      )}

      {/* Песочница (мастер-свитч, Linux-only) — предпосылка для host-exec. */}
      <EgressRow
        label={t('settings.agent.sandbox')}
        desc={shellSupported ? t('settings.agent.sandboxDesc') : t('settings.agent.linuxOnly')}
        value={flags.sandboxEnabled}
        // Выключая песочницу, persist сам сбрасывает shell (когерентность shell↔sandbox централизована).
        onChange={(v) => persist({ sandboxEnabled: v })}
        disabled={!shellSupported}
      />

      {/* Host-exec (shell/process/git) внутри песочницы. Требует sandbox + Linux; всегда Confirm. */}
      <EgressRow
        label={t('settings.agent.shell')}
        desc={shellAvailable ? t('settings.agent.shellDesc') : t('settings.agent.shellReq')}
        value={flags.shellEnable}
        onChange={(v) => persist({ shellEnable: v })}
        disabled={!shellAvailable}
      />
      {flags.shellEnable && shellAvailable && (
        <p className={styles.warnText}>{t('settings.agent.shellWarn')}</p>
      )}

      {/* Публичный web.fetch агента (снимает allowlist). Эффективен лишь при настроенном ai.web. */}
      <EgressRow
        label={t('settings.agent.publicFetch')}
        desc={t('settings.agent.publicFetchDesc')}
        value={flags.webAllowPublicFetch}
        onChange={(v) => persist({ webAllowPublicFetch: v })}
      />
      {flags.webAllowPublicFetch && (
        <p className={styles.warnText}>{t('settings.agent.publicFetchWarn')}</p>
      )}

      {saved && <span className={styles.okText}>{t('settings.web.saved')}</span>}
      {err && <p className={styles.warnText}>{t('settings.web.saveError', { msg: err })}</p>}
    </>
  );
}

/** Хост из URL для consent-баннера (или сырой ввод, если не парсится). */
function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/**
 * Политика эгресса ядра (срез 2 net.md): тоггл «офлайн» (E2) + per-feature opt-in (E6).
 * Применяется мгновенно (без Save) и переживает рестарт (E5, OS config-dir — вне vault/git).
 * Чат-бейдж local/offline (E9) и i18n-рендер отказов (AC-EGR-14) — следующим фронт-срезом.
 */
function EgressBlock() {
  const { t } = useTranslation();
  const [st, setSt] = useState<EgressState | null>(null);
  const [err, setErr] = useState('');

  useEffect(() => {
    let alive = true;
    void tauriApi.egress
      .getState()
      .then((s) => {
        if (alive) setSt(s);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  if (!st) return null;
  const apply = (p: Promise<EgressState>) => {
    setErr('');
    void p.then(setSt).catch((e: unknown) => setErr(String(e)));
  };

  return (
    <>
      <SectionHeader title={t('settings.egress.title')} sub={t('settings.egress.intro')} nested />
      <EgressRow
        label={t('settings.egress.offline')}
        desc={t('settings.egress.offlineDesc')}
        value={st.offline}
        onChange={(v) => apply(tauriApi.egress.setOffline(v))}
      />
      {(['chat', 'embed', 'probe'] as const).map((f) => (
        <EgressRow
          key={f}
          label={t(`settings.egress.${f}`)}
          desc={t(`settings.egress.${f}Desc`)}
          value={st[f]}
          onChange={(v) => apply(tauriApi.egress.setFeature(f, v))}
        />
      ))}
      {err && <p className={styles.warnText}>{t('settings.egress.saveError', { msg: err })}</p>}
    </>
  );
}

/** Строка политики: подпись + описание + сегмент Вкл/Выкл (паттерн `.seg`, как в «Редакторе»). */
function EgressRow(props: {
  label: string;
  desc: string;
  value: boolean;
  onChange: (v: boolean) => void;
  /** MEM-8b: задизейблить весь ряд (зависимая настройка недоступна — напр. консолидация без памяти). */
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const disabled = props.disabled ?? false;
  return (
    <section className={styles.group} aria-disabled={disabled || undefined}>
      <div className={styles.rowText}>
        <span className={styles.label}>{props.label}</span>
        <span className={styles.rowDesc}>{props.desc}</span>
      </div>
      <div className={styles.seg}>
        <button
          type="button"
          className={`${styles.segBtn} ${!props.value ? styles.on : ''}`}
          onClick={() => props.onChange(false)}
          aria-pressed={!props.value}
          disabled={disabled}
        >
          {t('settings.off')}
        </button>
        <button
          type="button"
          className={`${styles.segBtn} ${props.value ? styles.on : ''}`}
          onClick={() => props.onChange(true)}
          aria-pressed={props.value}
          disabled={disabled}
        >
          {t('settings.on')}
        </button>
      </div>
    </section>
  );
}

function Endpoint(props: {
  icon: ComponentType<{ size?: number; className?: string; 'aria-hidden'?: boolean }>;
  title: string;
  desc: string;
  url: string;
  model: string;
  onUrl: (v: string) => void;
  onModel: (v: string) => void;
  test: TestState;
  onTest: () => void;
}) {
  const { t } = useTranslation();
  const Ico = props.icon;
  return (
    <section className={styles.modelCard}>
      <div className={styles.modelHead}>
        <Ico size={16} className={styles.modelHeadIcon} aria-hidden />
        <div className={styles.modelHeadText}>
          <span className={styles.modelTitle}>{props.title}</span>
          <span className={styles.modelDesc}>{props.desc}</span>
        </div>
      </div>
      <label className={styles.field}>
        <span>{t('settings.aiSec.url')}</span>
        <input
          type="text"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          placeholder="http://127.0.0.1:8080"
          value={props.url}
          onChange={(e) => props.onUrl(e.target.value)}
        />
      </label>
      <label className={styles.field}>
        <span>{t('settings.aiSec.model')}</span>
        <input
          type="text"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          placeholder={t('settings.aiSec.modelPlaceholder')}
          value={props.model}
          onChange={(e) => props.onModel(e.target.value)}
        />
      </label>
      <div className={styles.testRow}>
        <button
          type="button"
          className={styles.testBtn}
          onClick={props.onTest}
          disabled={props.test.status === 'testing'}
        >
          {t('settings.aiSec.test')}
        </button>
        <TestBadge state={props.test} />
      </div>
    </section>
  );
}

function TestBadge({ state }: { state: TestState }) {
  const { t } = useTranslation();
  if (state.status === 'idle') return null;
  if (state.status === 'testing')
    return (
      <span className={styles.badge}>
        <Loader2 size={14} className={styles.spin} aria-hidden />
        {t('settings.aiSec.testing')}
      </span>
    );
  if (state.status === 'ok')
    return (
      <span className={`${styles.badge} ${styles.badgeOk}`}>
        <Check size={14} aria-hidden />
        {t('settings.aiSec.reachable')}
      </span>
    );
  return (
    <span className={`${styles.badge} ${styles.badgeFail}`} title={state.msg}>
      <AlertCircle size={14} aria-hidden />
      {t('settings.aiSec.unreachable')}
    </span>
  );
}

/**
 * Секция «Горячие клавиши» (кросс-план #11, слайс 4): список команд с их текущим хоткеем + захват новой
 * комбинации (capture-фаза window — раньше глобального `useKeymap`), сброс к дефолту, подсветка
 * конфликтов. Ремап/сброс идут через реестр команд (`commands.remap/resetKey`, персист в localStorage).
 */
function HotkeysSection() {
  const { t } = useTranslation();
  const [, force] = useReducer((x: number) => x + 1, 0);
  const [capturingId, setCapturingId] = useState<string | null>(null);

  // Перерисовка при изменении реестра (ремап/сброс/регистрация).
  useEffect(() => commands.subscribe(force), []);

  // Захват комбинации: слушаем на capture-фазе window — раньше глобального хоткей-хендлера, чтобы
  // нажатие не сработало как команда. Esc — отмена; ждём не-модификатор; требуем модификатор.
  useEffect(() => {
    if (!capturingId) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape') {
        setCapturingId(null);
        return;
      }
      if (e.key === 'Control' || e.key === 'Meta' || e.key === 'Alt' || e.key === 'Shift') return;
      if (!(e.ctrlKey || e.metaKey || e.altKey)) return;
      commands.remap(capturingId, eventToCombo(e));
      setCapturingId(null);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [capturingId]);

  const label = (c: { titleKey?: string; title: string }) => (c.titleKey ? t(c.titleKey) : c.title);
  const list = [...commands.list()].sort((a, b) => label(a).localeCompare(label(b)));

  // Подсчёт эффективных комбо → конфликт, если одна комбинация у ≥2 команд.
  const counts = new Map<string, number>();
  for (const c of list) {
    const k = commands.effectiveKey(c.id);
    if (k) counts.set(k, (counts.get(k) ?? 0) + 1);
  }

  return (
    <>
      <SectionHeader title={t('settings.hotkeys')} sub={t('settings.hk.intro')} />
      <ul className={styles.hkList}>
        {list.map((c) => {
          const key = commands.effectiveKey(c.id);
          const overridden = commands.userKeyFor(c.id) !== undefined;
          const conflict = key !== undefined && (counts.get(key) ?? 0) > 1;
          const capturing = capturingId === c.id;
          return (
            <li key={c.id} className={styles.hkRow}>
              <span className={styles.hkName}>{label(c)}</span>
              <div className={styles.hkRight}>
                {capturing ? (
                  <span className={styles.hkCapturing}>{t('settings.hk.press')}</span>
                ) : (
                  <kbd
                    className={`${styles.hkKey} ${conflict ? styles.hkConflict : ''}`}
                    title={conflict ? t('settings.hk.conflict') : undefined}
                    aria-label={key ? spellCombo(key) : undefined}
                  >
                    {key ? formatCombo(key) : '—'}
                  </kbd>
                )}
                <button
                  type="button"
                  className={styles.ghostBtn}
                  onClick={() => setCapturingId(capturing ? null : c.id)}
                >
                  {capturing ? t('settings.hk.cancel') : t('settings.hk.edit')}
                </button>
                {overridden && (
                  <button
                    type="button"
                    className={styles.hkReset}
                    onClick={() => commands.resetKey(c.id)}
                    title={t('settings.hk.reset')}
                    aria-label={t('settings.hk.reset')}
                  >
                    <RotateCcw size={14} aria-hidden />
                  </button>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </>
  );
}
