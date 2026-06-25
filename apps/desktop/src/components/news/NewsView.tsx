import { useEffect, useRef, useState } from 'react';
import {
  AlertTriangle,
  Check,
  ChevronRight,
  Eye,
  EyeOff,
  FilePlus2,
  Newspaper,
  Power,
  RefreshCw,
  Settings,
  ShieldCheck,
  WifiOff,
  X,
} from 'lucide-react';
import { OrbitIcon } from '../chrome/BrandGlyphs';
import { useTranslation } from 'react-i18next';

import { tauriApi, type NewsItem } from '../../lib/tauri-api';
import { useNewsStore } from '../../stores/news';
import { NewsDiagnostics } from './NewsDiagnostics';
import { NewsReader } from './NewsReader';
import styles from './NewsView.module.css';

/** Unix-секунды → относительное время («2 ч. назад») в локали UI. */
function relTime(ts: number, locale: string): string {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - ts);
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'short' });
  if (diff < 90) return rtf.format(-1, 'minute');
  if (diff < 3600) return rtf.format(-Math.floor(diff / 60), 'minute');
  if (diff < 86_400) return rtf.format(-Math.floor(diff / 3600), 'hour');
  if (diff < 30 * 86_400) return rtf.format(-Math.floor(diff / 86_400), 'day');
  return new Date(ts * 1000).toLocaleDateString(locale, { day: 'numeric', month: 'short' });
}

/** Скелетон карточек первого прогона (shimmer как в макете). */
function Skeleton() {
  return (
    <div>
      {[0, 1, 2, 3].map((i) => (
        <div key={i} className={styles.skel}>
          <div style={{ flex: 1 }}>
            <div className={styles.skLine} style={{ width: `${70 - i * 7}%` }} />
            <div className={styles.skLine} style={{ width: '92%', marginTop: 8 }} />
            <div className={styles.skLine} style={{ width: '40%', marginTop: 8, height: 9 }} />
          </div>
        </div>
      ))}
    </div>
  );
}

/**
 * Страница «Новости» (NF-5, спека `docs/specs/news-feed.md`, макет `news.jsx` хендоффа):
 * AI-сводка дня + чипы тем + рубрики с карточками. Состояния: consent-CTA (фича выкл,
 * AC-NF-7), первый прогон (скелетон), пустой день, ошибка прогона (прошлые данные целы),
 * «резюме недоступно» (LLM-фейл, AC-NF-10), офлайн-баннер. Заголовок ведёт на оригинал
 * (помечая прочитанным); встроенный reader с переводом — срез NF-6.
 */
export function NewsView() {
  const { t, i18n } = useTranslation();
  const {
    items,
    topics,
    run,
    config,
    sources,
    topic,
    unreadOnly,
    loading,
    refreshing,
    error,
    notice,
    load,
    refresh,
    markRead,
    toNote,
    setEnabled,
    setTopic,
    setUnreadOnly,
    clearNotice,
  } = useNewsStore();
  const [showErrors, setShowErrors] = useState(false);
  const [gearOpen, setGearOpen] = useState(false);
  // Облако тем свёрнуто по умолчанию при их избытке (фидбэк владельца: 47 чипов застилали экран).
  const [tagsExpanded, setTagsExpanded] = useState(false);
  // Этапный прогресс прогона (фидбэк 11.06): «Опрашиваю источники 7/16» вместо немого «Собираю…».
  const [runStage, setRunStage] = useState<{ stage: string; done: number; total: number } | null>(
    null,
  );
  // Доверенные хосты статей (per-host consent из ридера) — для управления в gear-меню.
  const [extraHosts, setExtraHosts] = useState<string[]>([]);
  const [offline, setOffline] = useState(false);
  /** Открытая в reader статья (NF-6); `null` — лента. */
  const [open, setOpen] = useState<NewsItem | null>(null);
  const gearRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void load();
    void tauriApi.egress.getState().then((s) => setOffline(s.offline));
  }, [load]);

  // Прогон асинхронный (джоба kind `newsfeed`): результат прилетает по `jobs:changed`.
  useEffect(() => {
    let unlisten = () => {};
    void tauriApi.events.onJobsChanged(() => void load()).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten();
  }, [load]);

  useEffect(() => {
    let off = () => {};
    void tauriApi.events
      .onNewsProgress((p) => setRunStage(p.stage === 'save' ? null : p))
      .then((fn) => {
        off = fn;
      });
    return () => off();
  }, []);

  // При открытии меню подтягиваем актуальный список доверенных хостов.
  useEffect(() => {
    if (!gearOpen) return;
    void tauriApi.news
      .getConfig()
      .then((c) => setExtraHosts(c.extraHosts))
      .catch(() => setExtraHosts([]));
  }, [gearOpen]);

  useEffect(() => {
    if (!gearOpen) return;
    const onDown = (e: MouseEvent) => {
      if (gearRef.current && !gearRef.current.contains(e.target as Node)) setGearOpen(false);
    };
    window.addEventListener('mousedown', onDown);
    return () => window.removeEventListener('mousedown', onDown);
  }, [gearOpen]);

  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(clearNotice, 3500);
    return () => clearTimeout(timer);
  }, [notice, clearNotice]);

  const sourceTitle = (id: string) => sources.find((s) => s.id === id)?.title ?? id;

  const openReader = (it: NewsItem) => {
    setOpen(it);
    if (!it.read) void markRead(it.id, true);
  };

  // ── фича выключена: onboarding-CTA + информированное согласие (AC-NF-7) ──
  if (config && !config.enabled) {
    const active = sources.filter((s) => s.enabled);
    return (
      <main className={styles.page} aria-label={t('news.title')}>
        <div className={styles.inner}>
          <div className={styles.cta}>
            <div className={styles.ctaGlyph}>
              <Newspaper size={28} aria-hidden />
            </div>
            <div className={styles.ctaTitle}>{t('news.ctaTitle')}</div>
            <div className={styles.ctaSub}>{t('news.ctaSub')}</div>
            <button type="button" className={styles.ctaBtn} onClick={() => void setEnabled(true)}>
              <Power size={17} aria-hidden />
              {t('news.enable')}
            </button>
            <div className={styles.consent}>
              <ShieldCheck size={16} aria-hidden />
              <div>
                <b>{t('news.consentLead')} </b>
                {t('news.consentBody', { count: active.length })}{' '}
                <span className={styles.srcs}>{active.map((s) => s.title).join(' · ')}</span>.
              </div>
            </div>
          </div>
        </div>
      </main>
    );
  }

  // ── reader (NF-6): открытая статья вместо ленты ──
  if (open) {
    return (
      <main className={`${styles.page} ${styles.pageReader}`} aria-label={t('news.title')}>
        <NewsReader
          item={open}
          sourceTitle={sourceTitle(open.sourceId)}
          onBack={() => setOpen(null)}
          onToNote={(id) => void toNote(id)}
        />
        {notice && (
          <div className={styles.toast} role="status">
            {t('news.noteCreated', { path: notice })}
          </div>
        )}
      </main>
    );
  }

  const unreadCount = items.filter((it) => !it.read).length;
  const groups = topics
    .map((tp) => ({ topic: tp, items: items.filter((it) => it.topic === tp) }))
    .filter((g) => g.items.length > 0);
  const filtersActive = topic !== null || unreadOnly;
  // При избытке тем показываем только первые TAG_COLLAPSE_AT + кнопку «Ещё N»; активный фильтр
  // (если он за порогом) всегда виден, чтобы выбранная тема не пропадала из ряда.
  const TAG_COLLAPSE_AT = 14;
  const tooManyTags = topics.length > TAG_COLLAPSE_AT;
  const visibleTopics =
    !tooManyTags || tagsExpanded
      ? topics
      : (() => {
          const head = topics.slice(0, TAG_COLLAPSE_AT);
          return topic && !head.includes(topic) ? [...head, topic] : head;
        })();
  const runFailed = run !== null && run.sourcesOk === 0 && run.errors.length > 0;
  // W-2: эндпоинт ОЦЕНКИ недоступен — backend кладёт ОДНУ строку «Анализатор новостей недоступен:
  // <url> …» в errors[] (только при сбое ВЫЗОВА, не на паре кривых JSON). Баннер ключуем на её
  // наличие (а НЕ на llmFailed>0, который растёт и от обычных парс-фейлов отдельных записей).
  const llmError = run?.errors.find((e) => e.startsWith('Анализатор новостей недоступен'));

  const card = (it: NewsItem) => (
    <div key={it.id} className={`${styles.card} ${it.read ? styles.cardRead : ''}`}>
      <div className={styles.ncUnread}>
        <i />
      </div>
      <div className={styles.ncMain}>
        <button
          type="button"
          className={styles.ncTitle}
          title={t('news.openArticle')}
          onClick={() => openReader(it)}
        >
          {it.titleRu}
          <ChevronRight aria-hidden />
        </button>
        {it.summaryRu ? (
          <div className={styles.ncSummary}>{it.summaryRu}</div>
        ) : (
          <div className={`${styles.ncSummary} ${styles.ncMissing}`}>
            {t('news.summaryMissing')}
          </div>
        )}
        <div className={styles.ncMeta}>
          <span className={styles.ncSrc}>{sourceTitle(it.sourceId)}</span>
          <span>·</span>
          <span>{relTime(it.publishedAt, i18n.language)}</span>
          <span className={styles.ncLang}>{it.langRu ? 'RU' : 'EN'}</span>
          <div className={styles.ncActions}>
            <button
              type="button"
              className={`${styles.ncAct} ${it.read ? styles.ncActOn : ''}`}
              title={it.read ? t('news.markUnread') : t('news.markRead')}
              aria-label={it.read ? t('news.markUnread') : t('news.markRead')}
              onClick={() => void markRead(it.id, !it.read)}
            >
              {it.read ? <EyeOff size={15} aria-hidden /> : <Eye size={15} aria-hidden />}
            </button>
            <button
              type="button"
              className={styles.ncAct}
              title={t('news.toNote')}
              aria-label={t('news.toNote')}
              onClick={() => void toNote(it.id)}
            >
              <FilePlus2 size={15} aria-hidden />
            </button>
          </div>
        </div>
      </div>
    </div>
  );

  return (
    <main className={styles.page} aria-label={t('news.title')}>
      <div className={styles.inner}>
        {offline && (
          <div className={styles.offlineBanner}>
            <WifiOff size={15} aria-hidden />
            {t('news.offlineBanner')}
          </div>
        )}
        {error && (
          <div className={styles.errBanner}>
            <AlertTriangle size={15} aria-hidden />
            {error === 'stalled' ? t('news.stalled') : error}
          </div>
        )}
        {/* W-2: анализатор новостей недоступен — видимый баннер с названным эндпоинтом (не молчаливо
            пустая лента). Показываем независимо от items.length. Строка из backend errors[] —
            операционная, RU (как и прочие news-ошибки в списке). */}
        {llmError && (
          <div className={styles.errBanner}>
            <AlertTriangle size={15} aria-hidden />
            {llmError}
          </div>
        )}

        <div className={styles.headRow}>
          {run && (
            <div className={styles.digest}>
              <div className={styles.ndHead}>
                <div className={styles.ndTitle}>
                  <OrbitIcon size={16} aria-hidden />
                  {t('news.digestTitle')}
                  <span className={styles.ndBadge}>AI</span>
                </div>
                <button
                  type="button"
                  className={`${styles.ndRefresh} ${refreshing ? styles.spinning : ''}`}
                  onClick={() => void refresh()}
                  disabled={refreshing}
                >
                  <RefreshCw size={14} aria-hidden />
                  {refreshing ? t('news.refreshing') : t('news.refresh')}
                </button>
              </div>
              {run.digestRu && <div className={styles.ndBody}>{run.digestRu}</div>}
              <div className={styles.ndMeta}>
                <span>{t('news.updated', { when: relTime(run.runAt, i18n.language) })}</span>
                <span>·</span>
                <span>{t('news.newCount', { count: run.itemsNew })}</span>
                {run.sourcesOk < run.sourcesTotal ? (
                  <>
                    <span>·</span>
                    <button
                      type="button"
                      className={styles.ndWarn}
                      onClick={() => setShowErrors((v) => !v)}
                    >
                      <AlertTriangle size={12} aria-hidden />
                      {t('news.sourcesOk', { ok: run.sourcesOk, total: run.sourcesTotal })}
                    </button>
                  </>
                ) : (
                  <>
                    <span>·</span>
                    <span>{t('news.sourcesOk', { ok: run.sourcesOk, total: run.sourcesTotal })}</span>
                  </>
                )}
              </div>
              {showErrors && run.errors.length > 0 && (
                <div className={styles.ndErrors}>
                  {run.errors.map((er) => (
                    <div key={er} className={styles.er}>
                      <X size={13} aria-hidden />
                      {er}
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
          {!run && <div style={{ flex: 1 }} />}
          <NewsDiagnostics lastRun={run} feedEmpty={items.length === 0 && !filtersActive} />
          <div className={styles.gearWrap} ref={gearRef}>
            <button
              type="button"
              className={styles.gearBtn}
              onClick={() => setGearOpen((v) => !v)}
              title={t('news.pageSettings')}
              aria-label={t('news.pageSettings')}
            >
              <Settings size={16} aria-hidden />
            </button>
            {gearOpen && (
              <div className={styles.menu} role="menu">
                <div className={styles.menuHead}>{t('news.title')}</div>
                <button
                  type="button"
                  className={styles.menuItem}
                  role="menuitem"
                  onClick={() => {
                    setGearOpen(false);
                    void setEnabled(false);
                  }}
                >
                  <Power size={15} aria-hidden />
                  {t('news.disable')}
                </button>
                {extraHosts.length > 0 && (
                  <>
                    <div className={styles.menuHead}>{t('news.trustedHosts')}</div>
                    {extraHosts.map((h) => (
                      <div key={h} className={styles.hostRow}>
                        <span className={styles.hostName}>{h}</span>
                        <button
                          type="button"
                          className={styles.hostDrop}
                          aria-label={t('news.dropHost', { host: h })}
                          title={t('news.dropHost', { host: h })}
                          onClick={() => {
                            void tauriApi.news
                              .disallowHost(h)
                              .then((c) => setExtraHosts(c.extraHosts))
                              .catch(() => {});
                          }}
                        >
                          <X size={13} aria-hidden />
                        </button>
                      </div>
                    ))}
                  </>
                )}
              </div>
            )}
          </div>
        </div>

        {/* Этап прогона видим и при «Обновить» поверх существующей ленты (фидбэк 11.06:
            «а где этапы?» — раньше строка жила только в первом прогоне без истории). */}
        {run && refreshing && runStage && (
          <div className={`${styles.rubric} ${styles.gathering}`}>
            <RefreshCw size={14} aria-hidden />
            <span>
              {t(`news.stage.${runStage.stage}`, {
                done: runStage.done,
                total: runStage.total,
                defaultValue: t('news.gathering'),
              })}
            </span>
          </div>
        )}

        {run && (items.length > 0 || filtersActive) && (
          <div className={styles.filters}>
            <button
              type="button"
              className={`${styles.chip} ${topic === null ? styles.chipOn : ''}`}
              onClick={() => setTopic(null)}
            >
              {t('news.all')}
              {!filtersActive && <span className={styles.cnt}>{unreadCount}</span>}
            </button>
            {visibleTopics.map((tp) => (
              <button
                type="button"
                key={tp}
                className={`${styles.chip} ${topic === tp ? styles.chipOn : ''}`}
                onClick={() => setTopic(tp)}
              >
                {tp}
              </button>
            ))}
            {tooManyTags && (
              <button
                type="button"
                className={`${styles.chip} ${styles.chipMore}`}
                onClick={() => setTagsExpanded((v) => !v)}
                aria-expanded={tagsExpanded}
              >
                {tagsExpanded
                  ? t('news.tagsLess')
                  : t('news.tagsMore', { count: topics.length - visibleTopics.length })}
              </button>
            )}
            <label className={styles.unreadToggle}>
              <button
                type="button"
                className={`${styles.switch} ${unreadOnly ? styles.switchOn : ''}`}
                role="switch"
                aria-checked={unreadOnly}
                onClick={() => setUnreadOnly(!unreadOnly)}
              >
                <i />
              </button>
              {t('news.unreadOnly')}
            </label>
          </div>
        )}

        {!run && (loading || refreshing) ? (
          <div>
            <div className={`${styles.rubric} ${styles.gathering}`}>
              <RefreshCw size={14} aria-hidden />
              <span>
                {runStage
                  ? t(`news.stage.${runStage.stage}`, {
                      done: runStage.done,
                      total: runStage.total,
                      defaultValue: t('news.gathering'),
                    })
                  : t('news.gathering')}
              </span>
            </div>
            <Skeleton />
          </div>
        ) : !run ? (
          <div className={styles.state}>
            <div className={styles.nsGlyph}>
              <Newspaper size={24} aria-hidden />
            </div>
            <div className={styles.nsTitle}>{t('news.neverTitle')}</div>
            <div className={styles.nsSub}>{t('news.neverSub')}</div>
            <button
              type="button"
              className={`${styles.ctaBtn} ${styles.ctaBtnSm}`}
              onClick={() => void refresh()}
            >
              <RefreshCw size={15} aria-hidden />
              {t('news.refresh')}
            </button>
          </div>
        ) : items.length === 0 && filtersActive ? (
          <div className={styles.state}>
            <div className={styles.nsGlyph}>
              <Check size={24} aria-hidden />
            </div>
            <div className={styles.nsTitle}>{t('news.caughtUpTitle')}</div>
            <div className={styles.nsSub}>{t('news.caughtUpSub')}</div>
          </div>
        ) : items.length === 0 && (runFailed || llmError) ? (
          // W-2: пустая лента при недоступном анализаторе (llmError) → состояние ОШИБКИ (а не «свежих
          // новостей нет»), согласованное с красным баннером выше (ревью W-2: иначе противоречие).
          <div className={styles.state}>
            <div className={`${styles.nsGlyph} ${styles.nsGlyphDanger}`}>
              <AlertTriangle size={24} aria-hidden />
            </div>
            <div className={styles.nsTitle}>{t('news.errorTitle')}</div>
            <div className={styles.nsSub}>{t('news.errorSub')}</div>
            <button
              type="button"
              className={`${styles.ctaBtn} ${styles.ctaBtnSm}`}
              onClick={() => void refresh()}
            >
              <RefreshCw size={15} aria-hidden />
              {t('news.retry')}
            </button>
          </div>
        ) : items.length === 0 && !llmError ? (
          <div className={styles.state}>
            <div className={styles.nsGlyph}>
              <Newspaper size={24} aria-hidden />
            </div>
            <div className={styles.nsTitle}>{t('news.emptyTitle')}</div>
            <div className={styles.nsSub}>{t('news.emptySub')}</div>
            <button
              type="button"
              className={`${styles.ctaBtn} ${styles.ctaBtnSm}`}
              onClick={() => void refresh()}
            >
              <RefreshCw size={15} aria-hidden />
              {t('news.refresh')}
            </button>
          </div>
        ) : (
          groups.map((g) => (
            <div key={g.topic}>
              <div className={styles.rubric}>
                {g.topic}
                <span className={styles.rc}>{g.items.length}</span>
              </div>
              <div className={styles.list}>{g.items.map(card)}</div>
            </div>
          ))
        )}
      </div>

      {notice && (
        <div className={styles.toast} role="status">
          {t('news.noteCreated', { path: notice })}
        </div>
      )}
    </main>
  );
}
