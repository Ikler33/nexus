import { useEffect, useRef, useState } from 'react';
import { Check, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi } from '../../lib/tauri-api';
import type { NewsConfig, NewsSource } from '../../lib/tauri-api';
import { useToastStore } from '../../stores/toast';
import styles from './SettingsView.module.css';

/** Заголовок секции (зеркало `SectionHeader` из SettingsView — дублируем минимально, не экспортим). */
function NewsHeader({ title, sub }: { title: string; sub?: string }) {
  return (
    <header className={styles.secHead}>
      <h2 className={styles.secTitle}>{title}</h2>
      {sub && <p className={styles.secSub}>{sub}</p>}
    </header>
  );
}

/** Ключевые слова в textarea ↔ массив: по строке или запятой, без пустых и дублей. */
function parseKeywords(raw: string): string[] {
  const out: string[] = [];
  for (const part of raw.split(/[\n,]/)) {
    const k = part.trim();
    if (k && !out.includes(k)) out.push(k);
  }
  return out;
}

/**
 * W-40 «Новости» — раздел настроек ленты. По образцу `AiSection`: на mount грузит конфиг ленты
 * (`news.getConfig`), реестр источников (`news.sources`) и AI-конфиг (`getAiConfig` — для подписи
 * url моделей под выбором). Все правки сохраняются МЕРЖЕМ в загруженный `cfg` (поля, которые секция
 * не редактирует — `enabled`/`extraHosts` при правке источников и т.п. — не затираются). Тост успех/
 * ошибка. latest-wins на асинхронных загрузках (reqRef). Вне Tauri работает на мок-данных.
 *
 * Диагностика/история прогонов/логи живут в ленте «Новости» (W-39) — здесь не дублируем.
 */
export function NewsSettingsSection() {
  const { t } = useTranslation();
  const addToast = useToastStore((s) => s.addToast);

  // Загруженный конфиг — источник истины для МЕРЖА: правки патчат его, не пересобирают с нуля.
  const [cfg, setCfg] = useState<NewsConfig | null>(null);
  const [sources, setSources] = useState<NewsSource[]>([]);
  // Url выбранных моделей (подписи под сегментом) — из ai-config, как SelfCheck/AiSection.
  const [fastUrl, setFastUrl] = useState('');
  const [chatUrl, setChatUrl] = useState('');
  // Локальный буфер textarea ключевых слов (массив ↔ текст), чтобы не терять курсор при наборе.
  const [keywordsText, setKeywordsText] = useState('');
  const [newHost, setNewHost] = useState('');
  const [saving, setSaving] = useState(false);
  // latest-wins: гасим ответ устаревшей загрузки, если секцию перемонтировали/перезагрузили.
  const reqRef = useRef(0);

  useEffect(() => {
    const req = ++reqRef.current;
    let alive = true;
    void Promise.all([
      tauriApi.news.getConfig(),
      tauriApi.news.sources(),
      tauriApi.settings.getAiConfig(),
    ]).then(([loaded, srcs, ai]) => {
      if (!alive || req !== reqRef.current) return;
      setCfg(loaded);
      setSources(srcs);
      setKeywordsText((loaded.keywords ?? []).join('\n'));
      setFastUrl(ai.fast?.url ?? '');
      setChatUrl(ai.chat?.url ?? '');
    });
    return () => {
      alive = false;
    };
  }, []);

  // Патчит загруженный конфиг (МЕРЖ), сохраняет через set_news_config, тостит. Возвращает применённый
  // конфиг от бэкенда (источник истины) — кладём его в стейт, чтобы следующая правка мержила свежее.
  const persist = async (patch: Partial<NewsConfig>) => {
    if (!cfg) return;
    const merged: NewsConfig = { ...cfg, ...patch };
    setCfg(merged); // оптимистично — UI отзывчив; откатывать не нужно (на ошибке тост + перезагрузка)
    setSaving(true);
    try {
      const applied = await tauriApi.news.setConfig(merged);
      setCfg(applied);
      addToast(t('settings.news.saved'), { kind: 'success' });
    } catch (e) {
      addToast(`${t('settings.news.saveError')}: ${String(e)}`, { kind: 'error' });
      // Перечитываем — стейт мог разойтись с диском после неудачной записи.
      void tauriApi.news.getConfig().then((reloaded) => {
        setCfg(reloaded);
        setKeywordsText((reloaded.keywords ?? []).join('\n'));
      });
    } finally {
      setSaving(false);
    }
  };

  if (!cfg) {
    return (
      <>
        <NewsHeader title={t('settings.news.title')} />
        <p className={styles.hint}>{t('settings.news.loading')}</p>
      </>
    );
  }

  // Действующее значение источника: переопределение из cfg.sources, иначе дефолт реестра.
  const sourceEnabled = (s: NewsSource): boolean => cfg.sources[s.id] ?? s.enabled;
  const modelPref: 'fast' | 'main' = cfg.modelPref === 'main' ? 'main' : 'fast';

  const setModel = (pref: 'fast' | 'main') => {
    if (modelPref === pref) return;
    void persist({ modelPref: pref });
  };

  const toggleSource = (s: NewsSource) => {
    // Источник истины чекбокса — cfg.sources (override поверх дефолта реестра), он корректно
    // откатывается перечитыванием cfg при ошибке сохранения. Без оптимистичной мутации
    // `sources`-массива — иначе при ошибке UI расходился бы с диском (MINOR из ревью W-40).
    void persist({ sources: { ...cfg.sources, [s.id]: !sourceEnabled(s) } });
  };

  const saveKeywords = () => {
    const parsed = parseKeywords(keywordsText);
    // Пустой ввод → null (= пресет по умолчанию), иначе массив (отличаем «не трогал» от «очистил»).
    void persist({ keywords: parsed.length ? parsed : null });
  };

  const addHost = () => {
    const h = newHost.trim().toLowerCase();
    if (!h || cfg.extraHosts.includes(h)) {
      setNewHost('');
      return;
    }
    setNewHost('');
    void persist({ extraHosts: [...cfg.extraHosts, h] });
  };

  const removeHost = (host: string) => {
    void persist({ extraHosts: cfg.extraHosts.filter((h) => h !== host) });
  };

  return (
    <>
      <NewsHeader title={t('settings.news.title')} sub={t('settings.news.intro')} />

      {/* Выбор модели пайплайна новостей — главный элемент W-40. Подписи = url разрешённых моделей. */}
      <section className={styles.group}>
        <div className={styles.rowText}>
          <span className={styles.label}>{t('settings.news.model')}</span>
          <span className={styles.rowDesc}>{t('settings.news.modelHint')}</span>
        </div>
        <div className={styles.seg}>
          <button
            type="button"
            className={`${styles.segBtn} ${modelPref === 'fast' ? styles.on : ''}`}
            onClick={() => setModel('fast')}
            aria-pressed={modelPref === 'fast'}
            disabled={saving}
          >
            {t('settings.news.modelFast')}
          </button>
          <button
            type="button"
            className={`${styles.segBtn} ${modelPref === 'main' ? styles.on : ''}`}
            onClick={() => setModel('main')}
            aria-pressed={modelPref === 'main'}
            disabled={saving}
          >
            {t('settings.news.modelMain')}
          </button>
        </div>
        <p className={styles.hint}>
          {modelPref === 'fast'
            ? fastUrl || t('settings.news.modelUnset')
            : chatUrl || t('settings.news.modelUnset')}
        </p>
      </section>

      {/* Источники реестра — чекбокс-лист; тоггл пишет переопределение в cfg.sources. */}
      <section className={styles.group}>
        <span className={styles.label}>{t('settings.news.sources')}</span>
        <div className={styles.skillsList}>
          {sources.map((s) => (
            <label key={s.id} className={styles.newsSourceRow}>
              <input
                type="checkbox"
                checked={sourceEnabled(s)}
                onChange={() => toggleSource(s)}
                disabled={saving}
                aria-label={s.title}
              />
              <span className={styles.newsSourceName}>{s.title}</span>
            </label>
          ))}
        </div>
      </section>

      {/* Ключевые слова фильтра — textarea (строка/запятая ↔ массив), сохраняется по «Применить»/blur. */}
      <section className={styles.group}>
        <div className={styles.rowText}>
          <span className={styles.label}>{t('settings.news.keywords')}</span>
          <span className={styles.rowDesc}>{t('settings.news.keywordsHint')}</span>
        </div>
        <textarea
          className={styles.newsInput}
          rows={3}
          spellCheck={false}
          value={keywordsText}
          onChange={(e) => setKeywordsText(e.target.value)}
          onBlur={saveKeywords}
          placeholder={t('settings.news.keywordsPlaceholder')}
          aria-label={t('settings.news.keywords')}
        />
        <div className={styles.saveBar}>
          <button
            type="button"
            className={styles.ghostBtn}
            onClick={saveKeywords}
            disabled={saving}
          >
            {t('settings.news.keywordsApply')}
          </button>
        </div>
      </section>

      {/* Доп. хосты статей (per-host consent ридера) — список со снятием + добавление. */}
      <section className={styles.group}>
        <div className={styles.rowText}>
          <span className={styles.label}>{t('settings.news.extraHosts')}</span>
          <span className={styles.rowDesc}>{t('settings.news.extraHostsHint')}</span>
        </div>
        {cfg.extraHosts.length > 0 && (
          <div className={styles.skillsList}>
            {cfg.extraHosts.map((host) => (
              <div key={host} className={styles.newsHostRow}>
                <span className={styles.newsHostName}>{host}</span>
                <button
                  type="button"
                  className={styles.skillBtn}
                  onClick={() => removeHost(host)}
                  disabled={saving}
                  aria-label={`${t('settings.news.removeHost')}: ${host}`}
                >
                  <X size={13} aria-hidden /> {t('settings.news.removeHost')}
                </button>
              </div>
            ))}
          </div>
        )}
        <input
          className={styles.newsInput}
          type="text"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          value={newHost}
          onChange={(e) => setNewHost(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              addHost();
            }
          }}
          placeholder={t('settings.news.addHostPlaceholder')}
          aria-label={t('settings.news.extraHosts')}
        />
        <div className={styles.saveBar}>
          <button
            type="button"
            className={styles.ghostBtn}
            onClick={addHost}
            disabled={saving || !newHost.trim()}
          >
            <Check size={14} aria-hidden /> {t('settings.news.addHost')}
          </button>
        </div>
      </section>

      <p className={styles.hint}>{t('settings.news.diagHint')}</p>
    </>
  );
}
