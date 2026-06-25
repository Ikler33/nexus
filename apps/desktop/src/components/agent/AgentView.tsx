import { useEffect, useRef, useState } from 'react';
import {
  AlertTriangle,
  ArrowUp,
  Check,
  ChevronRight,
  FilePlus2,
  FileText,
  GitBranch,
  ListChecks,
  Pause,
  Play,
  RotateCcw,
  Settings,
  Share2,
  ShieldCheck,
  Square,
  Terminal,
  X,
} from 'lucide-react';
import { OrbitIcon } from '../chrome/BrandGlyphs';
import { useTranslation } from 'react-i18next';

import { BrandThinking } from '../chrome/BrandThinking';
import { AgentHistory } from './AgentHistory';
import { describeStep } from './describeStep';
import { ExecGraph } from './ExecGraph';
import { Markdown } from '../common/Markdown';
import { useAgentStore, sessionStatus } from '../../stores/agent';
import { useWorkspaceStore } from '../../stores/workspace';
import { useToastStore } from '../../stores/toast';
import type { AgentPerms, AgentStep, AgentTurn, ChangesetFile } from '../../stores/agent';
import { lineDiff, type DiffLine } from '../../lib/diff';
import { tauriApi, type AgentFileStatus } from '../../lib/tauri-api';
import styles from './AgentView.module.css';

/** Доступные модели для отображаемого селектора (per-run политика UI; реальный выбор — конфиг бэка). */
const MODELS = ['qwen3:35b', 'llama3.3', 'gpt-5'] as const;

/**
 * Вкладка Агента (UI-1b) — полноэкранный агентский воркспейс на контракте UI-1a (`Channel<AgentStreamEvent>`).
 * Шапка (модель/автономность/права/контекст-бар) · лента шагов (стрим токенов ассистента + раскрываемые
 * tool-вызовы/результаты + дифы) · Changeset (per-file apply/reject + bulk → `decisions[]` → `agent_approve`)
 * · композер (→ `agent_run`) · правый dock (План — реальные шаги; Граф — реальный граф выполнения ExecGraph;
 *   Отчёт — из `final`).
 *
 * Все доки питаются ЖИВЫМИ данными хода: PlanLive — `turn.steps`, ExecGraph — `turn.steps`/`subagents`/
 * `execItems`/`report` (вертикальное дерево-таймлайн, заменил фейковый демо-ResearchGraph), Отчёт —
 * `final`/`researchReport`.
 */
export function AgentView() {
  const { t } = useTranslation();
  const toast = useToastStore((s) => s.addToast);

  const turns = useAgentStore((s) => s.turns);
  const autonomy = useAgentStore((s) => s.autonomy);
  const model = useAgentStore((s) => s.model);
  const perms = useAgentStore((s) => s.perms);
  const context = useAgentStore((s) => s.context);
  const approving = useAgentStore((s) => s.approving);

  const run = useAgentStore((s) => s.run);
  const setAutonomy = useAgentStore((s) => s.setAutonomy);
  const setModel = useAgentStore((s) => s.setModel);
  const setPerm = useAgentStore((s) => s.setPerm);
  const setFileDecision = useAgentStore((s) => s.setFileDecision);
  const setAllDecisions = useAgentStore((s) => s.setAllDecisions);
  const approve = useAgentStore((s) => s.approve);
  const pause = useAgentStore((s) => s.pause);
  const resume = useAgentStore((s) => s.resume);
  const cancel = useAgentStore((s) => s.cancel);
  const undo = useAgentStore((s) => s.undo);
  const newSession = useAgentStore((s) => s.newSession);

  const [settingsOpen, setSettingsOpen] = useState(false);
  const [aside, setAside] = useState<'plan' | 'graph' | 'report' | null>('plan');
  const [input, setInput] = useState('');
  const gearRef = useRef<HTMLDivElement>(null);

  const lastTurn = turns.length ? turns[turns.length - 1] : null;
  const status = sessionStatus(turns);
  const runId = lastTurn?.runId ?? null;
  const report = lastTurn?.report ?? null;
  const active = status === 'running' || status === 'paused' || status === 'awaiting';
  const started = turns.length > 0;

  // Закрытие меню настроек по клику снаружи (как макет).
  useEffect(() => {
    if (!settingsOpen) return;
    const onDown = (e: MouseEvent) => {
      if (gearRef.current && !gearRef.current.contains(e.target as Node)) setSettingsOpen(false);
    };
    window.addEventListener('mousedown', onDown);
    return () => window.removeEventListener('mousedown', onDown);
  }, [settingsOpen]);

  const submit = () => {
    const q = input.trim();
    if (!q || active) return;
    run(q);
    setInput('');
  };

  const onNewSession = () => {
    if (active) return;
    newSession();
    setInput('');
  };

  const doUndo = async () => {
    const n = await undo();
    toast(n > 0 ? t('agent.undone', { count: n }) : t('agent.undoneNone'), {
      kind: n > 0 ? 'success' : 'info',
    });
  };

  const ctxPct = context && context.window > 0
    ? Math.min(100, Math.round((context.used / context.window) * 100))
    : 0;
  const fmtK = (n: number) => (n >= 1000 ? `${Math.round(n / 1000)}k` : `${n}`);

  return (
    <main className={styles.agentv}>
      {/* ── Шапка ── */}
      <header className={styles.head}>
        <BrandThinking size={18} animate={active} />
        <div className={styles.title}>{t('agent.title')}</div>
        <span className={styles.sid}>
          {runId != null ? `#${runId} · ` : ''}
          {t('agent.session')}
        </span>
        {context && (
          <span className={styles.ctx} title={t('agent.contextUsage')}>
            <span className={styles.ctxLabel}>{t('agent.context')}</span>
            <span className={styles.ctxBar}>
              <i style={{ width: `${ctxPct}%` }} />
            </span>
            <span className={styles.ctxN}>
              {fmtK(context.used)} / {fmtK(context.window)}
            </span>
          </span>
        )}
        <div className={styles.spacer} />
        <button
          type="button"
          className={styles.chip}
          onClick={onNewSession}
          disabled={active}
          title={t('agent.newSession')}
        >
          <BrandThinking size={13} animate={false} />
          {t('agent.newSession')}
        </button>
        <div className={styles.gear} ref={gearRef}>
          <button
            type="button"
            className={`${styles.gearBtn} ${settingsOpen ? styles.gearActive : ''}`}
            onClick={() => setSettingsOpen((v) => !v)}
            title={t('agent.settings')}
            aria-label={t('agent.settings')}
            aria-expanded={settingsOpen}
          >
            <Settings size={16} aria-hidden />
          </button>
          {settingsOpen && (
            <div className={styles.settings} role="menu">
              <div className={styles.asSec}>{t('agent.model')}</div>
              <div className={styles.asSeg}>
                {MODELS.map((m) => (
                  <button
                    key={m}
                    type="button"
                    className={model === m ? styles.segOn : ''}
                    onClick={() => setModel(m)}
                    disabled={active}
                  >
                    {m}
                  </button>
                ))}
              </div>
              <div className={styles.asSec}>{t('agent.autonomy')}</div>
              <div className={styles.asSeg} role="radiogroup" aria-label={t('agent.autonomy')}>
                <button
                  type="button"
                  role="radio"
                  aria-checked={autonomy === 'confirm'}
                  className={autonomy === 'confirm' ? styles.segOn : ''}
                  onClick={() => setAutonomy('confirm')}
                  disabled={active}
                >
                  {t('agent.confirm')}
                </button>
                <button
                  type="button"
                  role="radio"
                  aria-checked={autonomy === 'auto'}
                  className={autonomy === 'auto' ? styles.segOn : ''}
                  onClick={() => setAutonomy('auto')}
                  disabled={active}
                >
                  {t('agent.auto')}
                </button>
              </div>
              <div className={styles.asSec}>{t('agent.perms')}</div>
              <div className={styles.asPerms}>
                {(['read', 'write', 'web'] as (keyof AgentPerms)[]).map((k) => (
                  <label key={k} className={styles.asPerm}>
                    <span>{t(`agent.${k}`)}</span>
                    <button
                      type="button"
                      role="switch"
                      aria-checked={perms[k]}
                      aria-label={t(`agent.${k}`)}
                      className={`${styles.setSwitch} ${perms[k] ? styles.switchOn : ''}`}
                      onClick={() => setPerm(k, !perms[k])}
                      disabled={active}
                    >
                      <i />
                    </button>
                  </label>
                ))}
              </div>
              <div className={styles.asFoot}>
                <ShieldCheck size={13} aria-hidden />
                {t('agent.sandbox')}
              </div>
            </div>
          )}
        </div>
      </header>

      {/* ── Тело: левый сайдбар истории + лента + правый dock ── */}
      <div className={styles.body}>
        <AgentHistory />
        <div className={styles.session}>
          <div className={styles.scroll}>
            {!started ? (
              <div className={styles.empty}>
                <BrandThinking size={34} animate={false} />
                <div className={styles.emptyTitle}>{t('agent.empty.title')}</div>
                <div className={styles.emptyHint}>{t('agent.empty.hint')}</div>
              </div>
            ) : (
              <>
                {/* Мультитёрн: лента ходов сессии (каждое сообщение = новый ход, прошлое НЕ стирается). */}
                {turns.map((turn, i) => (
                  <TurnView
                    key={turn.key}
                    turn={turn}
                    isLast={i === turns.length - 1}
                    autonomy={autonomy}
                    approving={approving}
                    onFile={setFileDecision}
                    onBulk={(d) => {
                      setAllDecisions(d);
                      toast(
                        d === 'applied'
                          ? t('agent.changeset.applyToast')
                          : t('agent.changeset.rejectToast'),
                      );
                    }}
                    onApprove={() => void approve()}
                    onPause={() => void pause()}
                    onResume={() => void resume()}
                    onCancel={() => void cancel()}
                    onUndo={() => void doUndo()}
                  />
                ))}
              </>
            )}
          </div>

          {/* Композер */}
          <div className={styles.composer}>
            <div className={styles.box}>
              <span className={styles.prompt}>❯</span>
              <textarea
                className={styles.boxInput}
                rows={1}
                value={input}
                placeholder={t('agent.composer.placeholder')}
                aria-label={t('agent.composer.placeholder')}
                disabled={active}
                onChange={(e) => setInput(e.target.value)}
                onInput={(e) => {
                  const el = e.currentTarget;
                  el.style.height = 'auto';
                  el.style.height = `${Math.min(el.scrollHeight, 168)}px`;
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault();
                    submit();
                  }
                }}
              />
              <button
                type="button"
                className={styles.send}
                disabled={!input.trim() || active}
                onClick={submit}
                title={t('agent.composer.send')}
                aria-label={t('agent.composer.send')}
              >
                <ArrowUp size={15} aria-hidden />
              </button>
            </div>
            <div className={styles.foot}>
              <span className={styles.footTag}>
                <BrandThinking size={12} animate={active} />
                {model}
              </span>
              <span className={styles.footTag}>{autonomy === 'confirm' ? t('agent.confirm') : t('agent.auto')}</span>
              <span className={styles.footTip}>{t('agent.composer.tip')}</span>
            </div>
          </div>
        </div>

        {/* Правый dock */}
        {aside && (
          <aside className={styles.dock}>
            <div className={styles.dockH}>
              <span>
                {aside === 'plan'
                  ? t('agent.dock.plan')
                  : aside === 'graph'
                    ? t('agent.dock.graph')
                    : t('agent.dock.report')}
              </span>
              <button
                type="button"
                className={styles.dockX}
                onClick={() => setAside(null)}
                aria-label={t('agent.dock.close')}
              >
                <X size={14} aria-hidden />
              </button>
            </div>
            <div className={styles.dockScroll}>
              {aside === 'plan' ? (
                <PlanLive />
              ) : aside === 'graph' ? (
                <GraphDock />
              ) : (
                <ReportPane
                  report={report}
                  research={lastTurn?.researchReport ?? null}
                  onToNote={() => toast(t('agent.report.savedToast'), { kind: 'success' })}
                />
              )}
            </div>
          </aside>
        )}

        {/* Рейл переключения dock-панелей */}
        <nav className={styles.rail} aria-label={t('agent.title')}>
          <button
            type="button"
            className={`${styles.railBtn} ${aside === 'plan' ? styles.railOn : ''}`}
            onClick={() => setAside((a) => (a === 'plan' ? null : 'plan'))}
            title={t('agent.dock.plan')}
            aria-label={t('agent.dock.plan')}
          >
            <ListChecks size={17} aria-hidden />
          </button>
          <button
            type="button"
            className={`${styles.railBtn} ${aside === 'graph' ? styles.railOn : ''}`}
            onClick={() => setAside((a) => (a === 'graph' ? null : 'graph'))}
            title={t('agent.dock.graph')}
            aria-label={t('agent.dock.graph')}
          >
            <Share2 size={18} aria-hidden />
          </button>
          <button
            type="button"
            className={`${styles.railBtn} ${aside === 'report' ? styles.railOn : ''}`}
            onClick={() => setAside((a) => (a === 'report' ? null : 'report'))}
            title={t('agent.dock.report')}
            aria-label={t('agent.dock.report')}
          >
            <FileText size={17} aria-hidden />
          </button>
        </nav>
      </div>
    </main>
  );
}

// ── Один ход диалога (task пользователя + ответ/действия агента) ───────────────────────────────────

interface TurnViewProps {
  turn: AgentTurn;
  /** Последний ход = активный/свежезавершённый: его контролы и аппрув-кнопки интерактивны. */
  isLast: boolean;
  autonomy: 'confirm' | 'auto';
  approving: boolean;
  onFile: (actionId: number, decision: 'applied' | 'rejected') => void;
  onBulk: (decision: 'applied' | 'rejected') => void;
  onApprove: () => void;
  onPause: () => void;
  onResume: () => void;
  onCancel: () => void;
  onUndo: () => void;
}

function TurnView({
  turn,
  isLast,
  autonomy,
  approving,
  onFile,
  onBulk,
  onApprove,
  onPause,
  onResume,
  onCancel,
  onUndo,
}: TurnViewProps) {
  const { t } = useTranslation();
  const status = turn.status;
  const active = status === 'running' || status === 'paused' || status === 'awaiting';

  return (
    <>
      {/* Сообщение пользователя (для первого хода — задача сессии) */}
      <div className={`${styles.msg} ${styles.msgUser}`}>
        <div className={styles.who}>{t('agent.who.task')}</div>
        <div className={styles.task}>{turn.task}</div>
      </div>

      {/* Ответ ассистента (склейка assistantToken). Во время стрима — плейн-текст (raw, быстро,
          markdown по живому дёргал бы вёрстку, как в чате); по завершении хода — финальный
          markdown-рендер (как ChatView: stream plain → final md). */}
      {(turn.assistantText || active) && (
        <div className={`${styles.msg} ${styles.msgBot}`}>
          <div className={styles.who}>
            <BrandThinking size={14} animate={active} />
            {t('agent.who.agent')}
          </div>
          {active || !turn.assistantText ? (
            <div className={styles.reply}>{turn.assistantText}</div>
          ) : (
            <Markdown content={turn.assistantText} className={styles.replyMd} />
          )}
        </div>
      )}

      {/* Лента шагов (tool-вызовы/результаты) */}
      {turn.steps.length > 0 && (
        <div className={styles.steps}>
          {turn.steps.map((st) => (
            <details
              key={st.id}
              className={`${styles.step} ${st.result == null ? styles.stepRun : st.isError ? styles.stepErr : styles.stepOk}`}
              open={st.result == null}
            >
              <summary className={styles.stepHead}>
                <span className={styles.stIc}>
                  {st.result == null ? (
                    <BrandThinking size={14} />
                  ) : st.isError ? (
                    <X size={13} aria-hidden />
                  ) : (
                    <Check size={13} aria-hidden />
                  )}
                </span>
                <span className={styles.stMain}>
                  <span className={styles.stLabel}>
                    <ChevronRight size={12} className={styles.stTw} aria-hidden />
                    <span className={styles.stKind}>{describeStep(st, t)}</span>
                  </span>
                </span>
                <span className={`${styles.stTag} ${st.result == null ? styles.tagRun : ''}`}>
                  {st.result == null ? t('agent.steps.running') : t('agent.steps.result')}
                </span>
              </summary>
              <div className={styles.stSub}>
                <div className={styles.stAct}>
                  <span className={styles.stActK}>{st.kind}</span> {st.args}
                </div>
                {st.result != null && (
                  <pre className={`${styles.toolOut} ${st.isError ? styles.toolErr : ''}`}>
                    {st.result}
                  </pre>
                )}
              </div>
            </details>
          ))}
        </div>
      )}

      {/* Changeset (proposal/diff). Аппрув активен только у последнего хода в статусе awaiting. */}
      {turn.changeset.length > 0 && (
        <Changeset
          files={turn.changeset}
          proposed={proposedContentByPath(turn.steps)}
          autonomy={autonomy}
          awaiting={isLast && status === 'awaiting'}
          approving={isLast && approving}
          active={active}
          onFile={onFile}
          onBulk={onBulk}
          onApprove={onApprove}
        />
      )}

      {/* Ошибка хода */}
      {turn.error && <div className={styles.error}>{turn.error}</div>}

      {/* Контролы прогона — только у последнего хода (пауза/продолжить/стоп/откат) */}
      {isLast && (active || status === 'done' || status === 'cancelled') && (
        <div className={styles.controls}>
          <span className={styles.statusPill} data-status={status}>
            {t(`agent.status.${status}`)}
          </span>
          <div className={styles.spacer} />
          {status === 'paused' ? (
            <button type="button" className={styles.ctrlBtn} onClick={onResume}>
              <Play size={14} aria-hidden /> {t('agent.controls.resume')}
            </button>
          ) : status === 'running' || status === 'awaiting' ? (
            <button type="button" className={styles.ctrlBtn} onClick={onPause}>
              <Pause size={14} aria-hidden /> {t('agent.controls.pause')}
            </button>
          ) : null}
          {active && (
            <button type="button" className={styles.ctrlBtn} onClick={onCancel}>
              <Square size={13} aria-hidden /> {t('agent.controls.cancel')}
            </button>
          )}
          {(status === 'done' || status === 'cancelled') && (
            <button type="button" className={styles.ctrlBtn} onClick={onUndo}>
              <RotateCcw size={13} aria-hidden /> {t('agent.controls.undo')}
            </button>
          )}
        </div>
      )}
    </>
  );
}

// ── Changeset (поверхность аппрува) ──────────────────────────────────────────────────────────────

interface ChangesetProps {
  files: ChangesetFile[];
  /** W-15: предложенный контент по vault-rel пути (из tool-вызовов хода) — для inline-диффа. */
  proposed: Map<string, string>;
  autonomy: 'confirm' | 'auto';
  awaiting: boolean;
  approving: boolean;
  /** Ход активен (running/paused/awaiting) — гейт анимации авто-бейджа Castor. */
  active: boolean;
  onFile: (actionId: number, decision: 'applied' | 'rejected') => void;
  onBulk: (decision: 'applied' | 'rejected') => void;
  onApprove: () => void;
}

/** W-15: предложенный контент по пути из tool-вызовов хода (`note.create`/`note.edit` несут
 *  `{path, content}`). Последний вызов на путь побеждает. Для inline-диффа в окне подтверждения. */
function proposedContentByPath(steps: AgentStep[]): Map<string, string> {
  const m = new Map<string, string>();
  for (const st of steps) {
    if (st.kind !== 'note.create' && st.kind !== 'note.edit') continue;
    try {
      const a = JSON.parse(st.args) as { path?: string; content?: string };
      if (a.path && typeof a.content === 'string') m.set(a.path, a.content);
    } catch {
      /* кривой args — пропускаем (диффа нет, счётчики ±N остаются) */
    }
  }
  return m;
}

function Changeset({
  files,
  proposed,
  autonomy,
  awaiting,
  approving,
  active,
  onFile,
  onBulk,
  onApprove,
}: ChangesetProps) {
  const { t } = useTranslation();
  const auto = autonomy === 'auto';
  // W-15: какой файл раскрыт для inline-диффа + кэш вычисленных диффов (current с диска ⟷ proposed).
  const [openDiff, setOpenDiff] = useState<string | null>(null);
  const [diffCache, setDiffCache] = useState<Record<string, DiffLine[]>>({});
  const toggleDiff = (path: string, status: AgentFileStatus) => {
    if (openDiff === path) {
      setOpenDiff(null);
      return;
    }
    setOpenDiff(path);
    if (diffCache[path]) return;
    const next = proposed.get(path) ?? '';
    // Новый файл — текущего контента нет → чистый add-дифф (без lineDiff('',…), который дал бы
    // ложную ведущую пустую `del`-строку, ревью W-15). Правка — читаем текущее с диска.
    if (status === 'new') {
      const lines: DiffLine[] =
        next === '' ? [] : next.split('\n').map((text) => ({ type: 'add', text }));
      setDiffCache((c) => ({ ...c, [path]: lines }));
      return;
    }
    void tauriApi.vault
      .readFile(path)
      .catch(() => '')
      .then((current) => {
        setDiffCache((c) => ({ ...c, [path]: lineDiff(current, next) }));
      });
  };
  const totAdd = files.reduce((a, f) => a + f.add, 0);
  const totDel = files.reduce((a, f) => a + f.del, 0);
  const pending = auto ? 0 : files.filter((f) => f.decision === undefined && f.actionId >= 0).length;

  // После решения (confirm-режим, ход ушёл из awaiting и нет нерешённых) — СВОРАЧИВАЕМ большую
  // интерактивную карточку в компактную строку-итог: окно «разрешения» уходит, мёртвых кнопок нет
  // (баг: карточка зависала после нажатия и «вела» вёрстку). Применённые правки видны в шагах хода.
  // `pending===0`: на паузе с нерешённым changeset карточка остаётся полной — resume вернёт кнопки.
  if (!auto && !awaiting && pending === 0) {
    const applied = files.filter((f) => f.decision === 'applied').length;
    const rejected = files.filter((f) => f.decision === 'rejected').length;
    const summary = [
      applied > 0 ? t('agent.changeset.resolvedApplied', { count: applied }) : null,
      rejected > 0 ? t('agent.changeset.resolvedRejected', { count: rejected }) : null,
    ]
      .filter(Boolean)
      .join(' · ');
    return (
      <div className={`${styles.changeset} ${styles.csResolved}`}>
        <div className={styles.csH}>
          <GitBranch size={14} className={styles.csIc} aria-hidden />
          <span className={styles.csT}>{t('agent.changeset.title')}</span>
          {summary && <span className={styles.csSum}>{summary}</span>}
        </div>
      </div>
    );
  }

  return (
    <div className={styles.changeset}>
      <div className={styles.csH}>
        <GitBranch size={14} className={styles.csIc} aria-hidden />
        <span className={styles.csT}>{t('agent.changeset.title')}</span>
        <span className={styles.csSum}>{t('agent.changeset.files', { count: files.length })}</span>
        <span className={styles.csAdd}>+{totAdd}</span>
        <span className={styles.csDel}>−{totDel}</span>
        {auto ? (
          <span className={styles.csAuto}>
            <BrandThinking size={13} animate={active} />
            {t('agent.changeset.autoBadge')}
          </span>
        ) : (
          <div className={styles.csBulk}>
            <button
              type="button"
              className={`${styles.csBk} ${styles.csBkApply}`}
              onClick={() => onBulk('applied')}
              disabled={!awaiting || approving}
            >
              {t('agent.changeset.applyAll')}
            </button>
            <button
              type="button"
              className={styles.csBk}
              onClick={() => onBulk('rejected')}
              disabled={!awaiting || approving}
            >
              {t('agent.changeset.reject')}
            </button>
          </div>
        )}
      </div>
      <div className={styles.csFiles}>
        {files.map((f) => {
          const decision = auto ? 'applied' : f.decision;
          const isExec = f.kind === 'exec';
          // ACP-EXEC: exec-строки рисуются как команда `$ cmd` (без ±строк/диффа). inline-дифф —
          // только для файловых правок (note.create/edit несут контент).
          const hasDiff = !isExec && proposed.has(f.path);
          const diffOpen = openDiff === f.path;
          return (
            <div key={`${f.path}:${f.actionId}`} className={styles.cfBlock}>
            <div
              className={`${styles.csFile} ${decision === 'applied' ? styles.csApplied : decision === 'rejected' ? styles.csRejected : ''}`}
            >
              {isExec ? (
                // ACP-EXEC: командная строка exec-стилем — `$ cmd` + ярлык «Выполнить команду», без ±/диффа.
                <>
                  <span className={styles.cfIc}>
                    <Terminal size={14} aria-hidden />
                  </span>
                  <code className={styles.cfCmd} title={f.path}>
                    <span className={styles.cfCmdDollar} aria-hidden>
                      ${' '}
                    </span>
                    {f.path}
                  </code>
                  <span className={styles.cfStat}>{t('agent.changeset.execLabel')}</span>
                </>
              ) : (
                <>
                  <span className={styles.cfIc}>
                    {f.status === 'new' ? (
                      <FilePlus2 size={14} aria-hidden />
                    ) : (
                      <FileText size={14} aria-hidden />
                    )}
                  </span>
                  <span className={styles.cfPath} title={f.path}>
                    {f.path}
                  </span>
                  <span className={styles.cfStat}>
                    {f.status === 'new' ? t('agent.changeset.new') : t('agent.changeset.edit')}
                  </span>
                  <span className={styles.cfCounts}>
                    <b className={styles.csAdd}>+{f.add}</b>
                    {f.del ? <b className={styles.csDel}> −{f.del}</b> : null}
                  </span>
                </>
              )}
              <div className={styles.cfActs}>
                {/* W-15: inline-дифф контента (а не только ±N) — раскрывается по клику. */}
                {hasDiff && (
                  <button
                    type="button"
                    className={`${styles.cfBtn} ${diffOpen ? styles.cfDiffOn : ''}`}
                    onClick={() => toggleDiff(f.path, f.status)}
                    title={t('agent.changeset.toggleDiff')}
                    aria-label={t('agent.changeset.toggleDiff')}
                    aria-expanded={diffOpen}
                  >
                    <ChevronRight
                      size={13}
                      aria-hidden
                      className={diffOpen ? styles.cfDiffChevOpen : undefined}
                    />
                  </button>
                )}
                {decision === 'applied' ? (
                  <span className={`${styles.cfBadge} ${styles.cfOk}`}>
                    {auto ? <OrbitIcon size={12} aria-hidden /> : <Check size={12} aria-hidden />}
                    {auto ? t('agent.changeset.autoMark') : t('agent.changeset.applied')}
                  </span>
                ) : decision === 'rejected' ? (
                  <span className={`${styles.cfBadge} ${styles.cfNo}`}>
                    {t('agent.changeset.rejected')}
                  </span>
                ) : (
                  <>
                    <button
                      type="button"
                      className={`${styles.cfBtn} ${styles.cfApply}`}
                      onClick={() => onFile(f.actionId, 'applied')}
                      disabled={!awaiting || approving}
                      title={t('agent.changeset.apply')}
                      aria-label={t('agent.changeset.apply')}
                    >
                      <Check size={13} aria-hidden />
                    </button>
                    <button
                      type="button"
                      className={styles.cfBtn}
                      onClick={() => onFile(f.actionId, 'rejected')}
                      disabled={!awaiting || approving}
                      title={t('agent.changeset.rejectOne')}
                      aria-label={t('agent.changeset.rejectOne')}
                    >
                      <X size={13} aria-hidden />
                    </button>
                  </>
                )}
              </div>
            </div>
            {/* W-15: раскрытый inline-дифф (current с диска ⟷ proposed из tool-args). */}
            {diffOpen && (
              <pre className={styles.cfDiff} aria-label={t('agent.changeset.toggleDiff')}>
                {diffCache[f.path] ? (
                  diffCache[f.path].length === 0 ||
                  diffCache[f.path].every((d) => d.type === 'same') ? (
                    <div className={styles.dEmpty}>{t('agent.changeset.diffEmpty')}</div>
                  ) : (
                    diffCache[f.path].map((d, i) => (
                      <div
                        key={i}
                        className={
                          d.type === 'add'
                            ? styles.dAdd
                            : d.type === 'del'
                              ? styles.dDel
                              : styles.dSame
                        }
                      >
                        <span className={styles.dGut} aria-hidden>
                          {d.type === 'add' ? '+' : d.type === 'del' ? '−' : ' '}
                        </span>
                        {d.text}
                      </div>
                    ))
                  )
                ) : (
                  <div className={styles.dEmpty}>{t('agent.changeset.diffLoading')}</div>
                )}
              </pre>
            )}
            </div>
          );
        })}
      </div>
      <div className={styles.csFoot}>
        {auto
          ? t('agent.changeset.footAuto')
          : pending > 0
            ? t('agent.changeset.footPending', { count: pending })
            : t('agent.changeset.footHandled')}
        {!auto && awaiting && (
          <button
            type="button"
            className={styles.csConfirm}
            onClick={onApprove}
            disabled={approving}
          >
            <Check size={13} aria-hidden /> {t('agent.changeset.confirmApprove')}
          </button>
        )}
      </div>
    </div>
  );
}

// ── Правый dock: План (W-14 — реальные шаги прогона) ────────────────────────────────────────────

/**
 * W-14: План = РЕАЛЬНЫЕ шаги активного хода (его tool-вызовы из стора). Раньше была статичная демо-
 * заглушка (ST-G6) — теперь каждый `toolCall` = пункт плана: `result==null` → выполняется
 * (BrandThinking), `isError` → ошибка, иначе → готово. Прогресс-бар = доля завершённых. Нет шагов
 * (ход без действий / только чат) → честный пустой стейт, без выдуманных пунктов.
 */
function PlanLive() {
  const { t } = useTranslation();
  const turns = useAgentStore((s) => s.turns);
  const turn = turns.length ? turns[turns.length - 1] : null;
  const steps = turn?.steps ?? [];

  if (steps.length === 0) {
    return (
      <div className={styles.plan}>
        <div className={styles.planEmpty}>{t('agent.plan.empty')}</div>
      </div>
    );
  }

  type S = 'done' | 'run' | 'err';
  const stateOf = (st: { result: string | null; isError: boolean }): S =>
    st.result == null ? 'run' : st.isError ? 'err' : 'done';
  const done = steps.filter((st) => st.result != null && !st.isError).length;
  const piCls: Record<S, string> = {
    done: styles.piDone,
    run: styles.piRun,
    err: styles.piErr,
  };
  return (
    <div className={styles.plan}>
      <div className={styles.planList}>
        {steps.map((st) => {
          const s = stateOf(st);
          return (
            <div key={st.id} className={`${styles.planItem} ${piCls[s]}`}>
              <span className={styles.planIc}>
                {s === 'done' ? (
                  <Check size={12} aria-hidden />
                ) : s === 'err' ? (
                  <AlertTriangle size={12} aria-hidden />
                ) : (
                  <BrandThinking size={13} />
                )}
              </span>
              <span className={styles.planLabel}>{st.kind}</span>
            </div>
          );
        })}
      </div>
      <div className={styles.planBar}>
        <i style={{ width: `${Math.round((done / steps.length) * 100)}%` }} />
      </div>
    </div>
  );
}

// ── Правый dock: дерево субагентов (W-24, живые данные) ──────────────────────────────────────────

/**
 * Содержимое дока «Граф» = РЕАЛЬНЫЙ граф выполнения (ExecGraph): вертикальное дерево-таймлайн над
 * состоянием последнего хода (steps=trunk, subagents=ветви, execItems/report=узлы). Заменяет фейковый
 * статичный ResearchGraph и СУБСУМИРУЕТ прежние SubagentTree (W-24, субагенты теперь = branch-узлы) и
 * ExecList (W-26, exec теперь = command-узлы). Рёбра только sequence+delegation (без выдуманной
 * причинности). NB: `turn.plan` (план) сюда НЕ входит — он в отдельной вкладке «План» (PlanLive).
 */
function GraphDock() {
  return <ExecGraph />;
}

// ── Правый dock: Отчёт (реальные данные из `final`) ──────────────────────────────────────────────

/**
 * Карточка отчёта deep-research (W-25): заголовок + путь + счётчики (источники/раунды) из последнего
 * хода (`turn.researchReport`, живые данные RES-5). «Открыть» → openFile(path) в редакторе.
 */
function ResearchReportCard({ doc }: { doc: NonNullable<AgentTurn['researchReport']> }) {
  const { t } = useTranslation();
  const openFile = useWorkspaceStore((s) => s.openFile);
  return (
    <div className={styles.repDoc}>
      <p className={styles.repP}>{doc.title}</p>
      <span className={styles.rcMeta}>
        {t('agent.research.sources', { count: doc.sourcesCount })} ·{' '}
        {t('agent.research.rounds', { count: doc.rounds })}
      </span>
      <div className={styles.repActs}>
        <button type="button" className={styles.artBtn} onClick={() => void openFile(doc.path)}>
          <FileText size={14} aria-hidden /> {t('agent.research.open')}
        </button>
      </div>
    </div>
  );
}

function ReportPane({
  report,
  research,
  onToNote,
}: {
  report: string | null;
  research: AgentTurn['researchReport'];
  onToNote: () => void;
}) {
  const { t } = useTranslation();
  if (!report && !research) return <div className={styles.reportEmpty}>{t('agent.report.empty')}</div>;
  return (
    <div className={styles.report}>
      {research && <ResearchReportCard doc={research} />}
      {report && (
        <div className={styles.repDoc}>
          <p className={styles.repP}>{report}</p>
        </div>
      )}
      {report && (
        <div className={styles.repActs}>
          <button type="button" className={styles.artBtn} onClick={onToNote}>
            <FilePlus2 size={14} aria-hidden /> {t('agent.report.toNote')}
          </button>
        </div>
      )}
    </div>
  );
}
