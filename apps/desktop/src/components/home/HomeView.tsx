import { useEffect, useMemo, useState, type ReactNode } from 'react';
import {
  ArrowLeftRight,
  ArrowUpRight,
  CalendarDays,
  ChevronRight,
  Clock,
  FileText,
  Folder,
  HelpCircle,
  LayoutGrid,
  Newspaper,
  PenLine,
  Plus,
  RefreshCw,
  Search,
  Share2,
  Sparkles,
  Star,
  Target,
  Trophy,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi, type AiConfigDto, type HeatDay } from '../../lib/tauri-api';
import { useHomeStore } from '../../stores/home';
import { usePrefsStore } from '../../stores/prefs';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { BrandThinking } from '../chrome/BrandThinking';
import styles from './HomeView.module.css';

const HEAT_WEEKS = 17;

/** Unix-секунды → относительное время в локали UI. */
function relTime(ts: number, locale: string): string {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - ts);
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'short' });
  if (diff < 90) return rtf.format(-1, 'minute');
  if (diff < 3600) return rtf.format(-Math.floor(diff / 60), 'minute');
  if (diff < 86_400) return rtf.format(-Math.floor(diff / 3600), 'hour');
  if (diff < 30 * 86_400) return rtf.format(-Math.floor(diff / 86_400), 'day');
  return new Date(ts * 1000).toLocaleDateString(locale, { day: 'numeric', month: 'short' });
}

/** `**жирные**` фрагменты LLM-текста → <strong> (как в макете, без markdown-движка). */
function renderBold(text: string): ReactNode[] {
  return text.split(/\*\*(.+?)\*\*/g).map((part, i) =>
    i % 2 === 1 ? <strong key={`${i}-${part.slice(0, 12)}`}>{part}</strong> : part,
  );
}

/** Ключ приветствия по локальному часу. */
function greetKey(hour: number): string {
  if (hour < 5) return 'night';
  if (hour < 12) return 'morning';
  if (hour < 18) return 'day';
  return 'evening';
}

/** Уровень ячейки heatmap из числа правок. */
function heatLevel(count: number): string {
  if (count <= 0) return '';
  if (count === 1) return 'l1';
  if (count === 2) return 'l2';
  if (count <= 4) return 'l3';
  return 'l4';
}

/** Детерминированная раскладка мини-графа: спираль золотого угла (виньетка, не настоящий sim). */
function miniLayout(n: number): { x: number; y: number }[] {
  const pts = [];
  for (let i = 0; i < n; i++) {
    const a = i * 2.39996;
    const r = 16 + 13.5 * Math.sqrt(i);
    pts.push({ x: 200 + r * Math.cos(a) * 1.35, y: 150 + r * Math.sin(a) * 0.92 });
  }
  return pts;
}

/**
 * HOME-дашборд (DP-1, макет `home.jsx`): приветствие + hero-поиск + «Продолжить» + быстрые
 * действия + секции карточек (AI-карты с teal-кантом, бейджем и thinking-оверлеем). Статика —
 * H1/H6, LLM-виджеты — кэш H3/H5 (refresh — фоновая джоба, готовность по `home:widget-updated`).
 */
export function HomeView() {
  const { t, i18n } = useTranslation();
  const locale = i18n.language;
  const userName = usePrefsStore((s) => s.userName);
  const info = useVaultStore((s) => s.info);
  const openPalette = useUIStore((s) => s.openPalette);
  const openGraph = useUIStore((s) => s.openGraph);
  const closeHome = useUIStore((s) => s.closeHome);
  const {
    data,
    activity,
    brief,
    questions,
    drift,
    stale,
    graph,
    generating,
    load,
    reloadWidget,
    refreshWidget,
  } = useHomeStore();
  const [ai, setAi] = useState<AiConfigDto | null>(null);
  const [revealed, setRevealed] = useState(false);

  useEffect(() => {
    void load();
    void tauriApi.settings.getAiConfig().then(setAi).catch(() => {});
    const raf = requestAnimationFrame(() => setRevealed(true));
    return () => cancelAnimationFrame(raf);
  }, [load]);

  useEffect(() => {
    let unlisten = () => {};
    void tauriApi.events
      .onWidgetUpdated((key) => void reloadWidget(key))
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten();
  }, [reloadWidget]);

  const openNote = (path: string) => {
    void useWorkspaceStore.getState().openFile(path);
    closeHome();
  };

  const newNote = async () => {
    const path = await useVaultStore.getState().createNote();
    openNote(path);
  };

  const dailyNote = async () => {
    const date = new Date().toISOString().slice(0, 10);
    const path = `Daily/${date}.md`;
    try {
      await tauriApi.vault.readFile(path);
    } catch {
      await tauriApi.vault.writeFile(path, `# ${date}\n\n`);
    }
    openNote(path);
  };

  const quickThought = async () => {
    try {
      await tauriApi.vault.readFile('Inbox.md');
    } catch {
      await tauriApi.vault.writeFile('Inbox.md', '# Inbox\n\n');
    }
    openNote('Inbox.md');
  };

  const heatCells = useMemo(() => {
    const byAgo = new Map<number, number>((activity?.heatmap ?? []).map((h: HeatDay) => [h.daysAgo, h.count]));
    const total = HEAT_WEEKS * 7;
    // Колонки недель слева направо к сегодня (grid-auto-flow: column заполняет по 7 строк).
    const cells = [];
    for (let i = 0; i < total; i++) {
      const ago = total - 1 - i;
      cells.push({ ago, count: byAgo.get(ago) ?? 0 });
    }
    return cells;
  }, [activity]);

  const trendPct = useMemo(() => {
    if (!activity || activity.prevWeek === 0) return null;
    return Math.round(((activity.week - activity.prevWeek) / activity.prevWeek) * 100);
  }, [activity]);

  const miniNodes = useMemo(() => {
    if (!graph || graph.nodes.length === 0) return null;
    const degree = new Map<number, number>();
    for (const e of graph.edges) {
      degree.set(e.source, (degree.get(e.source) ?? 0) + 1);
      degree.set(e.target, (degree.get(e.target) ?? 0) + 1);
    }
    const pts = miniLayout(graph.nodes.length);
    const pos = new Map(graph.nodes.map((n, i) => [n.id, pts[i]]));
    const maxDeg = Math.max(1, ...degree.values());
    return { pos, degree, maxDeg };
  }, [graph]);

  const today = new Date();
  const aiModel = ai?.chat?.model || (ai?.chat?.url ? new URL(ai.chat.url).host : null);
  const goalColors = [styles.cAccent, styles.cAi, styles.cSuccess];

  const aiCardHead = (
    icon: ReactNode,
    title: string,
    widgetKey: string,
  ) => (
    <div className={styles.cardHead}>
      <div className={styles.cardTitle}>
        {icon}
        {title}
        <span className={styles.aiBadge}>AI</span>
      </div>
      <button
        type="button"
        className={styles.cardAct}
        onClick={() => void refreshWidget(widgetKey)}
        disabled={Boolean(generating[widgetKey])}
      >
        <RefreshCw aria-hidden />
        {t('home.refresh')}
      </button>
    </div>
  );

  const thinking = (key: string) =>
    generating[key] ? (
      <div className={styles.aiThink}>
        <BrandThinking size={40} />
        <span className="mt-label">{t('home.thinking')}</span>
      </div>
    ) : null;

  return (
    <main className={styles.page} aria-label={t('home.title')}>
      <div className={styles.inner}>
        {/* ── приветствие ── */}
        <header className={`${styles.header} ${styles.reveal} ${revealed ? styles.revealIn : ''}`}>
          <svg className={styles.watermark} viewBox="0 0 32 32" fill="none" aria-hidden>
            <g stroke="currentColor" strokeWidth="1">
              <line x1="16" y1="16" x2="8" y2="8" />
              <line x1="16" y1="16" x2="25" y2="11" />
              <line x1="16" y1="16" x2="12" y2="25" />
            </g>
            <g fill="currentColor">
              <circle cx="16" cy="16" r="2" />
              <circle cx="8" cy="8" r="1.4" />
              <circle cx="25" cy="11" r="1.4" />
              <circle cx="12" cy="25" r="1.4" />
            </g>
          </svg>
          <div className={styles.greetWrap}>
            <div className={styles.greeting}>
              {t(`home.greet.${greetKey(today.getHours())}`)}
              {userName ? (
                <>
                  , <span className={styles.greetName}>{userName}</span>
                </>
              ) : null}
            </div>
            <div className={styles.sub}>
              {today.toLocaleDateString(locale, { weekday: 'long', day: 'numeric', month: 'long' })}
              {data ? ` · ${t('home.subNotes', { count: data.stats.notes })}` : ''}
              {activity ? ` · ${t('home.subChanges', { count: activity.changesToday })}` : ''}
            </div>
            <div className={styles.meta}>
              {aiModel && (
                <span className={`${styles.chip} ${styles.chipLive}`}>
                  <i className={styles.liveDot} />
                  {aiModel}
                </span>
              )}
              {info && (
                <span className={styles.chip}>
                  <Folder size={12} aria-hidden />
                  {info.root}
                </span>
              )}
              {activity && activity.streakDays > 0 && (
                <span className={styles.chip}>
                  <Star size={12} aria-hidden />
                  {t('home.streakChip', { count: activity.streakDays })}
                </span>
              )}
            </div>
          </div>
        </header>

        {/* ── hero-поиск ── */}
        <button type="button" className={styles.heroSearch} onClick={() => openPalette()}>
          <Search size={17} aria-hidden />
          <span className={styles.hsText}>{t('home.searchPlaceholder')}</span>
          <kbd className={styles.kbd}>⌘K</kbd>
        </button>

        {/* ── продолжить ── */}
        {activity?.continue && (
          <button
            type="button"
            className={styles.continue}
            onClick={() => openNote(activity.continue!.path)}
          >
            <div>
              <div className={styles.cEyebrow}>
                <ArrowUpRight size={13} aria-hidden />
                {t('home.continue')}
              </div>
              <div className={styles.cTitle}>
                {activity.continue.title ?? activity.continue.path}
              </div>
              {activity.continue.snippet && (
                <div className={styles.cSnippet}>{activity.continue.snippet}</div>
              )}
              <div className={styles.cMeta}>
                <span>{relTime(activity.continue.updatedAt, locale)}</span>
                <span>{t('home.words', { count: activity.continue.words })}</span>
              </div>
            </div>
            <span className={styles.cGo}>
              {t('home.continueGo')}
              <ChevronRight size={15} aria-hidden />
            </span>
          </button>
        )}

        {/* ── быстрые действия ── */}
        <div className={styles.quickActions}>
          <button type="button" className={styles.qa} onClick={() => void newNote()}>
            <Plus size={15} aria-hidden />
            {t('home.qaNew')}
          </button>
          <button type="button" className={styles.qa} onClick={() => void dailyNote()}>
            <Clock size={15} aria-hidden />
            {t('home.qaDaily')}
          </button>
          <button type="button" className={styles.qa} onClick={() => void quickThought()}>
            <PenLine size={15} aria-hidden />
            {t('home.qaThought')}
          </button>
          <button type="button" className={styles.qa} onClick={() => openGraph()}>
            <Share2 size={15} aria-hidden />
            {t('home.qaGraph')}
          </button>
        </div>

        {/* ── сводка ── */}
        <div className={styles.secLabel}>{t('home.secSummary')}</div>
        <div className={styles.grid2}>
          <div className={`${styles.card} ${styles.cardAi}`}>
            {aiCardHead(<Newspaper size={15} aria-hidden />, t('home.briefTitle'), 'daily_brief')}
            {brief?.content ? (
              <div className={styles.briefText}>{renderBold(brief.content)}</div>
            ) : (
              <div className={styles.cardEmpty}>{t('home.briefEmpty')}</div>
            )}
            {thinking('daily_brief')}
          </div>
          <div className={styles.card}>
            <div className={styles.cardHead}>
              <div className={styles.cardTitle}>
                <Clock size={15} aria-hidden />
                {t('home.recentTitle')}
              </div>
            </div>
            <div className={styles.hList}>
              {(data?.recent ?? []).map((n) => (
                <button
                  type="button"
                  key={n.path}
                  className={styles.hRow}
                  onClick={() => openNote(n.path)}
                >
                  <FileText size={15} aria-hidden />
                  <span className={styles.rBody}>
                    <span className={styles.rName}>{n.title ?? n.path}</span>
                    <span className={styles.rMeta}>
                      {t('home.words', { count: n.words })}
                    </span>
                  </span>
                  <span className={styles.rTime}>{relTime(n.updatedAt, locale)}</span>
                </button>
              ))}
              {data && data.recent.length === 0 && (
                <div className={styles.cardEmpty}>{t('home.recentEmpty')}</div>
              )}
            </div>
          </div>
        </div>

        {/* ── активность ── */}
        <div className={styles.secLabel}>{t('home.secActivity')}</div>
        <div className={styles.grid2}>
          <div className={styles.card}>
            <div className={styles.cardHead}>
              <div className={styles.cardTitle}>
                <CalendarDays size={15} aria-hidden />
                {t('home.activityTitle')}
              </div>
            </div>
            {activity && (
              <>
                <div className={styles.actMetrics}>
                  <div className={styles.actMetric}>
                    <div className={styles.amTop}>
                      <span className={styles.amVal}>{activity.week}</span>
                      {trendPct !== null && trendPct !== 0 && (
                        <span
                          className={`${styles.amTrend} ${trendPct > 0 ? styles.amUp : styles.amDown}`}
                        >
                          {trendPct > 0 ? '↑' : '↓'}
                          {Math.abs(trendPct)}%
                        </span>
                      )}
                    </div>
                    <div className={styles.amLabel}>{t('home.amWeek')}</div>
                  </div>
                  <div className={styles.actMetric}>
                    <div className={styles.amTop}>
                      <span className={styles.amVal}>{activity.changesToday}</span>
                    </div>
                    <div className={styles.amLabel}>{t('home.amToday')}</div>
                  </div>
                  <div className={styles.actMetric}>
                    <div className={styles.amTop}>
                      <span className={styles.amVal}>{activity.streakDays}</span>
                    </div>
                    <div className={styles.amLabel}>{t('home.amStreak')}</div>
                  </div>
                </div>
                {activity.bestStreak > 0 && (
                  <div className={styles.actGoal}>
                    <span className={styles.agIc}>
                      <Trophy size={15} aria-hidden />
                    </span>
                    <span className={styles.agText}>
                      {activity.streakDays >= activity.bestStreak
                        ? renderBold(t('home.goalRecord', { best: activity.bestStreak }))
                        : renderBold(
                            t('home.goalText', {
                              best: activity.bestStreak,
                              left: activity.bestStreak - activity.streakDays,
                            }),
                          )}
                    </span>
                    <span className={styles.agBar}>
                      <i
                        style={{
                          width: `${Math.min(100, Math.round((activity.streakDays / activity.bestStreak) * 100))}%`,
                        }}
                      />
                    </span>
                  </div>
                )}
                <div className={styles.heatGrid}>
                  {heatCells.map((c) => (
                    <i
                      key={c.ago}
                      className={`${styles.heatCell} ${c.count > 0 ? styles[heatLevel(c.count)] : ''}`}
                      title={`${c.count}`}
                    />
                  ))}
                </div>
                <div className={styles.heatLegend}>
                  {t('home.heatLess')}
                  <span className={styles.scale ?? 'scale'}>
                    <i className={styles.heatCell} style={{ width: 9, height: 9 }} />
                    <i className={`${styles.heatCell} ${styles.l1}`} style={{ width: 9, height: 9 }} />
                    <i className={`${styles.heatCell} ${styles.l2}`} style={{ width: 9, height: 9 }} />
                    <i className={`${styles.heatCell} ${styles.l3}`} style={{ width: 9, height: 9 }} />
                    <i className={`${styles.heatCell} ${styles.l4}`} style={{ width: 9, height: 9 }} />
                  </span>
                  {t('home.heatMore')}
                </div>
              </>
            )}
          </div>
          <div className={`${styles.card} ${styles.graphCard}`}>
            <div className={styles.cardHead}>
              <div className={styles.cardTitle}>
                <Share2 size={15} aria-hidden />
                {t('home.graphTitle')}
              </div>
              <button type="button" className={styles.cardAct} onClick={() => openGraph()}>
                {t('home.graphOpen')}
                <ChevronRight aria-hidden />
              </button>
            </div>
            <div
              className={styles.graphMini}
              role="button"
              tabIndex={0}
              onClick={() => openGraph()}
              onKeyDown={(e) => e.key === 'Enter' && openGraph()}
            >
              {miniNodes && graph && (
                <svg viewBox="0 0 400 300" width="100%" height="100%" aria-hidden>
                  {graph.edges.slice(0, 90).map((e, i) => {
                    const a = miniNodes.pos.get(e.source);
                    const b = miniNodes.pos.get(e.target);
                    if (!a || !b) return null;
                    return (
                      <line
                        key={`${e.source}-${e.target}-${i}`}
                        className={styles.gmEdge}
                        x1={a.x}
                        y1={a.y}
                        x2={b.x}
                        y2={b.y}
                        opacity={0.5}
                      />
                    );
                  })}
                  {graph.nodes.map((n) => {
                    const p = miniNodes.pos.get(n.id);
                    if (!p) return null;
                    const deg = miniNodes.degree.get(n.id) ?? 0;
                    const hub = deg >= miniNodes.maxDeg * 0.7 && deg > 1;
                    return (
                      <circle
                        key={n.id}
                        className={`${styles.gmNode} ${hub ? styles.gmHub : ''}`}
                        cx={p.x}
                        cy={p.y}
                        r={2 + Math.min(4, deg * 0.6)}
                      />
                    );
                  })}
                </svg>
              )}
              {graph && (
                <span className={styles.gmCta}>
                  {t('home.graphCta', { notes: graph.totalFiles, links: graph.edges.length })}
                  <ChevronRight size={11} aria-hidden />
                </span>
              )}
            </div>
          </div>
        </div>

        {/* ── проекты ── */}
        <div className={styles.secLabel}>{t('home.secProjects')}</div>
        <div className={styles.grid2}>
          <div className={styles.card}>
            <div className={styles.cardHead}>
              <div className={styles.cardTitle}>
                <Target size={15} aria-hidden />
                {t('home.goalsTitle')}
              </div>
            </div>
            <div className={styles.progList}>
              {(data?.goals ?? []).slice(0, 4).map((g, i) => (
                <div key={g.path}>
                  <div className={styles.progRow}>
                    <button
                      type="button"
                      className={styles.progName}
                      onClick={() => openNote(g.path)}
                    >
                      {g.title ?? g.path}
                    </button>
                    {g.progress !== null ? (
                      <span className={styles.progPct}>{g.progress}%</span>
                    ) : (
                      <span className={styles.noProg}>{t('home.noProgress')}</span>
                    )}
                  </div>
                  {g.progress !== null && (
                    <div className={styles.progTrack}>
                      <i
                        className={`${styles.progFill} ${goalColors[i % goalColors.length]}`}
                        style={{ width: `${g.progress}%` }}
                      />
                    </div>
                  )}
                </div>
              ))}
              {data && data.goals.length === 0 && (
                <div className={styles.cardEmpty}>{t('home.goalsEmpty')}</div>
              )}
            </div>
          </div>
          <div className={styles.card}>
            <div className={styles.cardHead}>
              <div className={styles.cardTitle}>
                <LayoutGrid size={15} aria-hidden />
                {t('home.statsTitle')}
              </div>
            </div>
            <div className={styles.statGrid}>
              <div className={styles.stat}>
                <div className={styles.statVal}>{data?.stats.notes ?? '—'}</div>
                <div className={styles.statLabel}>{t('home.statNotes')}</div>
              </div>
              <div className={styles.stat}>
                <div className={styles.statVal}>{activity?.changesToday ?? '—'}</div>
                <div className={styles.statLabel}>{t('home.statToday')}</div>
              </div>
              <div className={styles.stat}>
                <div className={styles.statVal}>{activity?.orphans ?? '—'}</div>
                <div className={styles.statLabel}>{t('home.statOrphans')}</div>
              </div>
              <div className={styles.stat}>
                <div className={styles.statVal}>{activity?.streakDays ?? '—'}</div>
                <div className={styles.statLabel}>{t('home.statStreak')}</div>
              </div>
            </div>
          </div>
        </div>

        {/* ── требует внимания ── */}
        <div className={styles.secLabel}>{t('home.secAttention')}</div>
        <div className={styles.grid2}>
          <div className={styles.card}>
            <div className={styles.cardHead}>
              <div className={styles.cardTitle}>
                <Clock size={15} aria-hidden />
                {t('home.staleTitle')}
              </div>
            </div>
            <div className={styles.hList}>
              {stale.slice(0, 5).map((s) => (
                <button
                  type="button"
                  key={s.path}
                  className={styles.staleRow}
                  onClick={() => openNote(s.path)}
                >
                  <i
                    className={`${styles.staleDot} ${s.severity === 'red' ? styles.hot : styles.warm}`}
                  />
                  <span className={styles.staleName}>{s.title ?? s.path}</span>
                  <span className={styles.staleDays}>
                    {t('home.staleDays', { count: s.ageDays })}
                  </span>
                  {s.action && (
                    <span className={styles.staleDo}>{t(`home.staleDo.${s.action}`)}</span>
                  )}
                </button>
              ))}
              {stale.length === 0 && (
                <div className={styles.cardEmpty}>{t('home.staleEmpty')}</div>
              )}
            </div>
          </div>
          <div className={`${styles.card} ${styles.cardAi}`}>
            {aiCardHead(
              <HelpCircle size={15} aria-hidden />,
              t('home.oqTitle'),
              'open_questions',
            )}
            <div className={styles.oqList}>
              {questions.map((q) => (
                <button
                  type="button"
                  key={q.question}
                  className={styles.oq}
                  onClick={() => openNote(q.path)}
                >
                  {q.question}
                </button>
              ))}
              {questions.length === 0 && (
                <div className={styles.cardEmpty}>{t('home.oqEmpty')}</div>
              )}
            </div>
            {thinking('open_questions')}
          </div>
        </div>

        {/* ── анализ ── */}
        <div className={styles.secLabel}>{t('home.secAnalysis')}</div>
        <div className={styles.gridFull}>
          <div className={`${styles.card} ${styles.cardAi}`}>
            {aiCardHead(
              <ArrowLeftRight size={15} aria-hidden />,
              t('home.driftTitle'),
              'context_drift',
            )}
            {drift ? (
              <div className={styles.driftText}>{renderBold(drift)}</div>
            ) : (
              <div className={styles.cardEmpty}>{t('home.driftEmpty')}</div>
            )}
            {thinking('context_drift')}
          </div>
        </div>

        <div className={styles.secLabel}>
          <Sparkles size={11} aria-hidden style={{ marginRight: -6 }} />
        </div>
      </div>
    </main>
  );
}
