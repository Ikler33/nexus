import { useEffect, useState } from 'react';
import { Activity, AlertTriangle, Check, ChevronRight, Download, History, RefreshCw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi, type NewsEndpointHealth, type NewsRun } from '../../lib/tauri-api';
import styles from './NewsView.module.css';

/** Unix-секунды → дата+время в локали UI (история прогонов — абсолютные метки, не «N ч. назад»). */
function absTime(ts: number, locale: string): string {
  return new Date(ts * 1000).toLocaleString(locale, {
    day: 'numeric',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/**
 * Панель «Диагностика» (W-39): даёт ВИДИМОСТЬ загрузки ленты — последний прогон (когда/новых/
 * источники/ошибки анализатора), история прогонов, «Проверить связь» с провайдером, «Скачать логи»,
 * и осмысленную причину пустоты (анализатор недоступен / источники не ответили / всё дедуплицировано).
 * Раскрывается по кнопке у шапки ленты; данные читаются лениво при открытии.
 *
 * `lastRun` — последний прогон из стора (та же шапка ленты); `feedEmpty` — лента пуста (для блока
 * «почему пусто»: показываем вероятную причину, а не молчим).
 */
export function NewsDiagnostics({
  lastRun,
  feedEmpty,
}: {
  lastRun: NewsRun | null;
  feedEmpty: boolean;
}) {
  const { t, i18n } = useTranslation();
  const [openPanel, setOpenPanel] = useState(false);
  const [runs, setRuns] = useState<NewsRun[]>([]);
  const [expandedRun, setExpandedRun] = useState<number | null>(null);
  const [health, setHealth] = useState<NewsEndpointHealth | null>(null);
  const [checking, setChecking] = useState(false);
  const [logNotice, setLogNotice] = useState<string | null>(null);

  // История подтягивается лениво при первом открытии (compact-список последних прогонов).
  useEffect(() => {
    if (!openPanel) return;
    void tauriApi.news
      .runs(10)
      .then(setRuns)
      .catch(() => setRuns([]));
  }, [openPanel]);

  // Тост «журнал сохранён / ошибка» самосбрасывается.
  useEffect(() => {
    if (!logNotice) return;
    const timer = setTimeout(() => setLogNotice(null), 3500);
    return () => clearTimeout(timer);
  }, [logNotice]);

  const testConn = () => {
    setChecking(true);
    // latest-wins (как SelfCheck): показываем результат последнего нажатия.
    void tauriApi.news
      .testEndpoint()
      .then(setHealth)
      .catch((e) =>
        setHealth({ ok: false, message: String(e), endpoint: '', latencyMs: 0 }),
      )
      .finally(() => setChecking(false));
  };

  const downloadLogs = () => {
    void tauriApi.news
      .exportLogs()
      .then((path) => {
        if (path) setLogNotice(t('news.diag.logsSaved', { path }));
      })
      .catch((e) => setLogNotice(t('news.diag.logsFailed', { error: String(e) })));
  };

  // Вероятная причина пустоты ленты по последнему прогону (приоритет: анализатор → источники → дедуп).
  const emptyReason = (): string => {
    if (!lastRun) return t('news.diag.emptyUnknown');
    // B12: структурное поле llmDown — эндпоинт берём из него, без regex по строке.
    if (lastRun.llmDown && !lastRun.llmDown.partial) {
      return lastRun.llmDown.endpoint
        ? t('news.diag.emptyLlm', { endpoint: lastRun.llmDown.endpoint })
        : t('news.diag.emptyLlmGeneric');
    }
    // @deprecated legacy-сниффер: записи news_runs до миграции 027 несут только RU-строку с
    // префиксом (URL выковыривается регексом) — держим, пока такие записи живы (ретенция 30 дней).
    const llmErr = lastRun.errors.find((e) => e.startsWith('Анализатор новостей недоступен'));
    if (llmErr) {
      const url = llmErr.match(/https?:\/\/[^\s—]+/)?.[0];
      return url
        ? t('news.diag.emptyLlm', { endpoint: url })
        : t('news.diag.emptyLlmGeneric');
    }
    if (lastRun.llmFailed > 0) return t('news.diag.emptyLlmGeneric');
    if (lastRun.sourcesOk === 0)
      return t('news.diag.emptySources', {
        ok: lastRun.sourcesOk,
        total: lastRun.sourcesTotal,
      });
    if (lastRun.itemsNew === 0) return t('news.diag.emptyDedup');
    return t('news.diag.emptyUnknown');
  };

  return (
    <div className={styles.diagWrap}>
      <button
        type="button"
        className={`${styles.gearBtn} ${openPanel ? styles.gearBtnOn : ''}`}
        onClick={() => setOpenPanel((v) => !v)}
        title={t('news.diag.open')}
        aria-label={t('news.diag.open')}
        aria-expanded={openPanel}
      >
        <Activity size={16} aria-hidden />
      </button>

      {openPanel && (
        <section className={styles.diagPanel} aria-label={t('news.diag.title')}>
          <div className={styles.diagHead}>
            <Activity size={15} aria-hidden />
            {t('news.diag.title')}
          </div>

          {/* «Почему пусто» — объясняем вероятную причину, а не молчим. */}
          {feedEmpty && (
            <div className={styles.diagEmpty}>
              <AlertTriangle size={14} aria-hidden />
              <div>
                <b>{t('news.diag.whyEmpty')}.</b> {emptyReason()}
              </div>
            </div>
          )}

          {/* Последний прогон. */}
          <div className={styles.diagSub}>{t('news.diag.lastRun')}</div>
          {lastRun ? (
            <div className={styles.diagStats}>
              <div>
                <span className={styles.diagKey}>{t('news.diag.when')}</span>
                <span>{absTime(lastRun.runAt, i18n.language)}</span>
              </div>
              <div>
                <span className={styles.diagKey}>{t('news.diag.newItems')}</span>
                <span>{lastRun.itemsNew}</span>
              </div>
              <div>
                <span className={styles.diagKey}>{t('news.diag.sources')}</span>
                <span>
                  {lastRun.sourcesOk}/{lastRun.sourcesTotal}
                </span>
              </div>
              <div>
                <span className={styles.diagKey}>{t('news.diag.llmFailed')}</span>
                <span>{lastRun.llmFailed}</span>
              </div>
              {lastRun.errors.length > 0 && (
                <div className={styles.diagErrors}>
                  <span className={styles.diagKey}>{t('news.diag.errors')}</span>
                  {lastRun.errors.map((er) => (
                    <div key={er} className={styles.er}>
                      <X size={12} aria-hidden />
                      {er}
                    </div>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <div className={styles.diagMuted}>{t('news.diag.never')}</div>
          )}

          {/* Действия: проверить связь + скачать логи. */}
          <div className={styles.diagActions}>
            <button
              type="button"
              className={styles.diagBtn}
              onClick={testConn}
              disabled={checking}
            >
              <RefreshCw size={14} className={checking ? styles.spinning : ''} aria-hidden />
              {checking ? t('news.diag.checking') : t('news.diag.testConn')}
            </button>
            <button type="button" className={styles.diagBtn} onClick={downloadLogs}>
              <Download size={14} aria-hidden />
              {t('news.diag.downloadLogs')}
            </button>
          </div>

          {/* Пилюля статуса связи (latest-wins). */}
          {health && (
            <div
              className={`${styles.diagPill} ${health.ok ? styles.diagPillOk : styles.diagPillBad}`}
              role="status"
            >
              {health.ok ? <Check size={13} aria-hidden /> : <X size={13} aria-hidden />}
              <span className={styles.diagPillLabel}>
                {health.ok ? t('news.diag.ok') : t('news.diag.fail')}
              </span>
              {health.endpoint && <span className={styles.diagPillEndpoint}>{health.endpoint}</span>}
              {health.ok && (
                <span className={styles.diagPillLatency}>
                  {t('news.diag.latency', { ms: health.latencyMs })}
                </span>
              )}
              {!health.ok && health.message && (
                <span className={styles.diagPillMsg}>{health.message}</span>
              )}
            </div>
          )}

          {/* История прогонов — компактный список (дата · +N · src ok/total · ошибки-раскрытие). */}
          <div className={styles.diagSub}>
            <History size={13} aria-hidden /> {t('news.diag.history')}
          </div>
          {runs.length === 0 ? (
            <div className={styles.diagMuted}>{t('news.diag.historyEmpty')}</div>
          ) : (
            <ul className={styles.diagHistory}>
              {runs.map((r) => {
                const expanded = expandedRun === r.runAt;
                return (
                  <li key={r.runAt} className={styles.diagRun}>
                    <button
                      type="button"
                      className={styles.diagRunRow}
                      onClick={() =>
                        setExpandedRun((cur) => (cur === r.runAt ? null : r.runAt))
                      }
                      disabled={r.errors.length === 0}
                      aria-expanded={r.errors.length > 0 ? expanded : undefined}
                    >
                      <span className={styles.diagRunWhen}>{absTime(r.runAt, i18n.language)}</span>
                      <span className={styles.diagRunNew}>+{r.itemsNew}</span>
                      <span
                        className={`${styles.diagRunSrc} ${
                          r.sourcesOk < r.sourcesTotal ? styles.diagRunSrcWarn : ''
                        }`}
                      >
                        {r.sourcesOk}/{r.sourcesTotal}
                      </span>
                      {r.errors.length > 0 && (
                        <span className={styles.diagRunErr}>
                          <AlertTriangle size={12} aria-hidden />
                          {r.errors.length}
                          <ChevronRight
                            size={12}
                            aria-hidden
                            className={expanded ? styles.diagChevOpen : ''}
                          />
                        </span>
                      )}
                    </button>
                    {expanded && r.errors.length > 0 && (
                      <div className={styles.diagRunErrors}>
                        {r.errors.map((er) => (
                          <div key={er} className={styles.er}>
                            <X size={12} aria-hidden />
                            {er}
                          </div>
                        ))}
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}

          {logNotice && (
            <div className={styles.diagToast} role="status">
              {logNotice}
            </div>
          )}
        </section>
      )}
    </div>
  );
}
