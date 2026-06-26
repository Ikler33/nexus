import { useEffect, useState } from 'react';
import {
  AlertTriangle,
  ArrowLeft,
  ExternalLink,
  FilePlus2,
  Info,
  MessageSquare,
  X,
} from 'lucide-react';
import { OrbitIcon } from '../chrome/BrandGlyphs';
import { useTranslation } from 'react-i18next';

import { logUi } from '../../lib/debug-log';
import { tauriApi, type NewsArticle, type NewsItem } from '../../lib/tauri-api';
import { RelatedNotesSection } from './RelatedNotesSection';
import styles from './NewsView.module.css';

/** Состояние загрузки статьи (поверх DTO бэкенда). */
type ArticleState = 'loading' | NewsArticle | { status: 'error'; message: string };

/** Unix-секунды → относительное время (копия хелпера NewsView — один экран, без общего модуля). */
function relTime(ts: number, locale: string): string {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - ts);
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'short' });
  if (diff < 90) return rtf.format(-1, 'minute');
  if (diff < 3600) return rtf.format(-Math.floor(diff / 60), 'minute');
  if (diff < 86_400) return rtf.format(-Math.floor(diff / 3600), 'hour');
  if (diff < 30 * 86_400) return rtf.format(-Math.floor(diff / 86_400), 'day');
  return new Date(ts * 1000).toLocaleDateString(locale, { day: 'numeric', month: 'short' });
}

/**
 * Reader статьи (NF-6, финальная итерация макета): полный RU-перевод in-app вместо ухода в
 * браузер; панель действий ВСЕГДА видна над текстом («К ленте / Сократить / В заметку /
 * Оригинал» — пожелание владельца: не уезжает при скролле); «Сократить» — тезисы on-demand
 * поверх полного текста. Хост вне политики эгресса → честный отказ + резюме + оригинал.
 */
export function NewsReader(props: {
  item: NewsItem;
  sourceTitle: string;
  onBack: () => void;
  onToNote: (id: number) => void;
}) {
  const { item, sourceTitle, onBack, onToNote } = props;
  const { t, i18n } = useTranslation();
  const [article, setArticle] = useState<ArticleState>('loading');
  const [summary, setSummary] = useState<'thinking' | string[] | null>(null);
  // Инкремент = повторная загрузка статьи (после «Разрешить хост», per-host consent NF-6-rev).
  const [reloadTick, setReloadTick] = useState(0);
  const [allowBusy, setAllowBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    setArticle('loading');
    setSummary(null);
    tauriApi.news
      .article(item.id)
      .then((a) => {
        if (alive) setArticle(a);
      })
      .catch((e: unknown) => {
        if (alive) setArticle({ status: 'error', message: String(e) });
      });
    return () => {
      alive = false;
    };
  }, [item.id, reloadTick]);

  // Хост статьи — для per-host consent в denied-состоянии (показываем, ЧТО именно разрешаем).
  const articleHost = (() => {
    try {
      return new URL(item.url).hostname;
    } catch {
      return null;
    }
  })();

  const allowHost = () => {
    if (!articleHost || allowBusy) return;
    setAllowBusy(true);
    logUi('news:allow-host', articleHost);
    tauriApi.news
      .allowHost(articleHost)
      .then(() => setReloadTick((n) => n + 1))
      .catch((e: unknown) => setArticle({ status: 'error', message: String(e) }))
      .finally(() => setAllowBusy(false));
  };

  // Esc возвращает в ленту (как выход из оверлеев).
  useEffect(() => {
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onBack();
    };
    window.addEventListener('keydown', onEsc);
    return () => window.removeEventListener('keydown', onEsc);
  }, [onBack]);

  const summarize = () => {
    if (summary === 'thinking') return;
    setSummary('thinking');
    tauriApi.news
      .summarize(item.id)
      .then((bullets) => setSummary(bullets))
      .catch(() => setSummary(null));
  };

  const ready = typeof article === 'object' && article.status === 'ready' ? article : null;
  const denied = typeof article === 'object' && article.status === 'denied' ? article : null;
  const error = typeof article === 'object' && article.status === 'error' ? article : null;

  return (
    <div className={styles.reader}>
      <div className={styles.readerBar}>
        <button type="button" className={styles.readerBack} onClick={onBack}>
          <ArrowLeft size={15} aria-hidden />
          {t('news.reader.back')}
        </button>
        <div className={styles.readerBarActions}>
          <button
            type="button"
            className={`${styles.readerAct} ${summary && summary !== 'thinking' ? styles.readerActOn : ''}`}
            onClick={summarize}
            // P1-15: нет тела статьи (article не `ready`: denied/error/loading) → сокращать нечего;
            // блокировка честна — клик без тела давал пустой summary/ошибку.
            disabled={summary === 'thinking' || !ready}
            title={!ready ? t('news.reader.summarizeUnavailable') : undefined}
          >
            <OrbitIcon size={15} aria-hidden />
            {t('news.reader.summarize')}
          </button>
          <button type="button" className={styles.readerAct} onClick={() => onToNote(item.id)}>
            <FilePlus2 size={15} aria-hidden />
            {t('news.toNote')}
          </button>
          <a
            className={styles.readerAct}
            href={item.url}
            target="_blank"
            rel="noreferrer noopener"
            onClick={(e) => {
              // В Tauri-вебвью target=_blank не открывает браузер → идём через opener.
              e.preventDefault();
              void tauriApi.external.open(item.url).catch(() => {});
            }}
          >
            <ExternalLink size={15} aria-hidden />
            {t('news.reader.original')}
          </a>
          {item.commentsUrl && (
            <a
              className={styles.readerAct}
              href={item.commentsUrl}
              target="_blank"
              rel="noreferrer noopener"
              onClick={(e) => {
                e.preventDefault();
                void tauriApi.external.open(item.commentsUrl!).catch(() => {});
              }}
            >
              <MessageSquare size={15} aria-hidden />
              {t('news.reader.discussion')}
            </a>
          )}
        </div>
      </div>

      <article className={styles.readerDoc}>
        <div className={styles.readerMeta}>
          <span className={styles.rmSrc}>{sourceTitle}</span>
          <span>·</span>
          <span>{relTime(item.publishedAt, i18n.language)}</span>
          <span className={styles.ncLang}>{item.langRu ? 'RU' : 'EN'}</span>
          {ready?.translated && (
            <>
              <span>·</span>
              <span className={styles.rmTrans}>
                <OrbitIcon size={11} aria-hidden />
                {t('news.reader.translated')}
              </span>
            </>
          )}
        </div>
        {sourceTitle && <div className={styles.readerKicker}>{sourceTitle}</div>}
        <h1 className={styles.readerTitle}>{item.titleRu}</h1>

        {summary === 'thinking' && (
          <div className={`${styles.readerSummary} ${styles.readerSummaryThinking}`}>
            <OrbitIcon size={16} aria-hidden className={styles.thinkSpin} />
            <span>{t('news.reader.summarizing')}</span>
          </div>
        )}
        {Array.isArray(summary) && summary.length > 0 && (
          <div className={styles.readerSummary}>
            <div className={styles.rsHead}>
              <OrbitIcon size={14} aria-hidden />
              {t('news.reader.brief')}
              <button
                type="button"
                className={styles.rsClose}
                onClick={() => setSummary(null)}
                title={t('news.reader.hide')}
                aria-label={t('news.reader.hide')}
              >
                <X size={13} aria-hidden />
              </button>
            </div>
            <ul className={styles.rsList}>
              {summary.map((s) => (
                <li key={s}>{s}</li>
              ))}
            </ul>
          </div>
        )}

        {item.summaryRu && <div className={styles.readerLede}>{item.summaryRu}</div>}

        <RelatedNotesSection itemId={item.id} />

        {article === 'loading' && (
          <div className={styles.readerLoading}>
            <OrbitIcon size={15} aria-hidden className={styles.thinkSpin} />
            <span>{t('news.reader.loading')}</span>
          </div>
        )}
        {error && (
          <div className={styles.errBanner}>
            <AlertTriangle size={15} aria-hidden />
            {error.message}
          </div>
        )}
        {denied && (
          <div className={styles.offlineBanner}>
            <AlertTriangle size={15} aria-hidden />
            <div className={styles.deniedBody}>
              {t('news.reader.denied', { message: denied.message })}
              {articleHost && (
                <div className={styles.allowRow}>
                  <button
                    type="button"
                    className={styles.allowBtn}
                    onClick={allowHost}
                    disabled={allowBusy}
                  >
                    {allowBusy
                      ? t('news.reader.allowing')
                      : t('news.reader.allowHost', { host: articleHost })}
                  </button>
                  <span className={styles.allowHint}>{t('news.reader.allowHint')}</span>
                </div>
              )}
            </div>
          </div>
        )}
        {ready?.paras.map((p) => (
          <p key={p.slice(0, 64)} className={styles.readerP}>
            {p}
          </p>
        ))}

        {(ready || denied) && (
          <div className={styles.readerFoot}>
            <Info size={13} aria-hidden />
            <span>
              {denied
                ? t('news.reader.footDenied')
                : ready?.translated
                  ? t('news.reader.footTranslated')
                  : t('news.reader.footOriginalRu')}
              {ready?.truncated ? ` ${t('news.reader.footTruncated')}` : ''}
            </span>
          </div>
        )}
      </article>
    </div>
  );
}
