import { useEffect, useReducer, useState } from 'react';
import {
  AlertCircle,
  Check,
  Cpu,
  Globe,
  Info,
  Keyboard,
  Loader2,
  Palette,
  Pencil,
  RotateCcw,
  X,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { changeLocale } from '../../i18n/setup';
import { commands, eventToCombo, formatCombo } from '../../lib/commands';
import { tauriApi } from '../../lib/tauri-api';
import type { EgressState, WebSearchConfig } from '../../lib/tauri-api';
import { usePrefsStore } from '../../stores/prefs';
import { ACCENTS, THEMES, useThemeStore } from '../../stores/theme';
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
  { id: 'general', icon: Globe, key: 'settings.general' },
  { id: 'editor', icon: Pencil, key: 'settings.editor' },
  { id: 'appearance', icon: Palette, key: 'settings.appearance' },
  { id: 'ai', icon: Cpu, key: 'settings.ai' },
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
          {section === 'general' && <GeneralSection />}
          {section === 'editor' && <EditorSection />}
          {section === 'appearance' && <AppearanceSection />}
          {section === 'ai' && <AiSection />}
          {section === 'hotkeys' && <HotkeysSection />}
          {section === 'about' && <AboutSection />}
        </div>
      </div>
    </div>
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
      <h2 className={styles.h2}>{t('settings.general')}</h2>
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
      <h2 className={styles.h2}>{t('settings.editor')}</h2>
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
      <h2 className={styles.h2}>{t('settings.appearance')}</h2>
      <section className={styles.group}>
        <span className={styles.label}>{t('tweaks.theme')}</span>
        <div className={styles.seg}>
          {THEMES.map((th) => (
            <button
              key={th}
              type="button"
              className={`${styles.segBtn} ${theme === th ? styles.on : ''}`}
              onClick={() => setTheme(th)}
            >
              {t(`tweaks.themes.${th}`)}
            </button>
          ))}
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
  const aiExplainRelations = usePrefsStore((s) => s.aiExplainRelations);
  const setAiExplainRelations = usePrefsStore((s) => s.setAiExplainRelations);
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
      <h2 className={styles.h2}>{t('settings.ai')}</h2>
      <p className={styles.hint}>{t('settings.aiSec.intro')}</p>

      <Endpoint
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
    void tauriApi.websearch.setConfig(next).then((applied) => {
      setCfg(applied);
      setUrl(applied.url);
      setSaved(true);
    });
  };

  return (
    <>
      <h2 className={styles.h2}>{t('settings.web.title')}</h2>
      <p className={styles.hint}>{t('settings.web.intro')}</p>
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
      <h2 className={styles.h2}>{t('settings.egress.title')}</h2>
      <p className={styles.hint}>{t('settings.egress.intro')}</p>
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
}) {
  const { t } = useTranslation();
  return (
    <section className={styles.group}>
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
        >
          {t('settings.off')}
        </button>
        <button
          type="button"
          className={`${styles.segBtn} ${props.value ? styles.on : ''}`}
          onClick={() => props.onChange(true)}
          aria-pressed={props.value}
        >
          {t('settings.on')}
        </button>
      </div>
    </section>
  );
}

function Endpoint(props: {
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
  return (
    <section className={styles.endpoint}>
      <h3 className={styles.subhead}>{props.title}</h3>
      <p className={styles.desc}>{props.desc}</p>
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
          className={styles.ghostBtn}
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
      <h2 className={styles.h2}>{t('settings.hotkeys')}</h2>
      <p className={styles.hint}>{t('settings.hk.intro')}</p>
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
