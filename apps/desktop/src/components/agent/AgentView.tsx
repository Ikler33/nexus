import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
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
  X,
} from 'lucide-react';
import { OrbitIcon } from '../chrome/BrandGlyphs';
import { useTranslation } from 'react-i18next';

import { BrandThinking } from '../chrome/BrandThinking';
import { useAgentStore, sessionStatus } from '../../stores/agent';
import { useToastStore } from '../../stores/toast';
import type { AgentPerms, AgentTurn, ChangesetFile } from '../../stores/agent';
import styles from './AgentView.module.css';

/** Доступные модели для отображаемого селектора (per-run политика UI; реальный выбор — конфиг бэка). */
const MODELS = ['qwen3:35b', 'llama3.3', 'gpt-5'] as const;

/**
 * Вкладка Агента (UI-1b) — полноэкранный агентский воркспейс на контракте UI-1a (`Channel<AgentStreamEvent>`).
 * Шапка (модель/автономность/права/контекст-бар) · лента шагов (стрим токенов ассистента + раскрываемые
 * tool-вызовы/результаты + дифы) · Changeset (per-file apply/reject + bulk → `decisions[]` → `agent_approve`)
 * · композер (→ `agent_run`) · правый dock (План/Граф — демо-структура; Отчёт — из `final`).
 *
 * Plan/ResearchGraph здесь — ДОКУМЕНТИРОВАННАЯ статичная демо-структура (контракт UI-1a не несёт plan-шагов
 * /graph-данных — только AssistantToken/ToolCall/ToolResult/ContextUsage/Proposal/Diff/Final/Error). Отчёт —
 * РЕАЛЬНЫЕ данные из события `final`.
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
        <BrandThinking size={18} />
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
          <BrandThinking size={13} />
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

      {/* ── Тело: лента + правый dock ── */}
      <div className={styles.body}>
        <div className={styles.session}>
          {!started ? (
            <div className={styles.empty}>
              <BrandThinking size={34} />
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
                <BrandThinking size={12} />
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
                <ResearchGraph />
              ) : (
                <ReportPane report={report} onToNote={() => toast(t('agent.report.savedToast'), { kind: 'success' })} />
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

      {/* Ответ ассистента (склейка assistantToken) */}
      {(turn.assistantText || active) && (
        <div className={`${styles.msg} ${styles.msgBot}`}>
          <div className={styles.who}>
            <BrandThinking size={14} />
            {t('agent.who.agent')}
          </div>
          <div className={styles.reply}>{turn.assistantText}</div>
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
                    <span className={styles.stKind}>{st.kind}</span>
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
          autonomy={autonomy}
          awaiting={isLast && status === 'awaiting'}
          approving={isLast && approving}
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
  autonomy: 'confirm' | 'auto';
  awaiting: boolean;
  approving: boolean;
  onFile: (actionId: number, decision: 'applied' | 'rejected') => void;
  onBulk: (decision: 'applied' | 'rejected') => void;
  onApprove: () => void;
}

function Changeset({ files, autonomy, awaiting, approving, onFile, onBulk, onApprove }: ChangesetProps) {
  const { t } = useTranslation();
  const auto = autonomy === 'auto';
  const totAdd = files.reduce((a, f) => a + f.add, 0);
  const totDel = files.reduce((a, f) => a + f.del, 0);
  const pending = auto ? 0 : files.filter((f) => f.decision === undefined && f.actionId >= 0).length;

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
            <BrandThinking size={13} />
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
          return (
            <div
              key={`${f.path}:${f.actionId}`}
              className={`${styles.csFile} ${decision === 'applied' ? styles.csApplied : decision === 'rejected' ? styles.csRejected : ''}`}
            >
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
              <div className={styles.cfActs}>
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

// ── Правый dock: Граф выполнения (демо-структура) ────────────────────────────────────────────────

/**
 * Граф выполнения — ДОКУМЕНТИРОВАННАЯ статичная демо-визуализация (research-задачи). Контракт UI-1a НЕ несёт
 * graph-данных, поэтому это эталон из дизайна (`agent-view.jsx::ResearchGraph`), помеченный demoNote.
 * Радиальная раскладка детерминирована (без backend-данных).
 */
function ResearchGraph() {
  const { t } = useTranslation();
  const cx = 150;
  const cy = 220;
  const R = 96;
  const D2R = Math.PI / 180;
  const rounds = [
    { a: -90, n: 14 },
    { a: -25, n: 8 },
    { a: 45, n: 7 },
    { a: 120, n: 12 },
    { a: 200, n: 6 },
  ];
  const edges: ReactNode[] = [];
  const hubs: ReactNode[] = [];
  const dots: ReactNode[] = [];
  let di = 0;
  rounds.forEach((rd, ri) => {
    const hx = cx + R * Math.cos(rd.a * D2R);
    const hy = cy + R * Math.sin(rd.a * D2R);
    edges.push(
      <line key={`e${ri}`} className={styles.rgEdge} x1={cx} y1={cy} x2={hx} y2={hy} />,
    );
    hubs.push(<circle key={`h${ri}`} className={styles.rgHub} cx={hx} cy={hy} r={6.5} />);
    hubs.push(
      <text key={`hl${ri}`} className={styles.rgLabel} x={hx} y={hy - 11} textAnchor="middle">
        {`R${ri + 1}`}
      </text>,
    );
    for (let k = 0; k < rd.n; k++) {
      const spread = rd.n > 1 ? (k / (rd.n - 1) - 0.5) * 96 : 0;
      const ang = (rd.a + spread) * D2R;
      const rr = 15 + (k % 3) * 5.5;
      dots.push(
        <circle
          key={`d${ri}-${k}`}
          className={styles.rgDot}
          cx={hx + rr * Math.cos(ang)}
          cy={hy + rr * Math.sin(ang)}
          r={2.4}
        />,
      );
      di++;
    }
  });
  return (
    <div className={styles.research}>
      <div className={styles.demoNote}>{t('agent.graph.demoNote')}</div>
      <div className={styles.rcH}>
        <span className={styles.rcMeta}>{t('agent.graph.meta')}</span>
      </div>
      <svg
        className={styles.rgSvg}
        viewBox="0 0 300 460"
        preserveAspectRatio="xMidYMid meet"
        fill="none"
        aria-hidden
      >
        <g>{edges}</g>
        <circle className={styles.rgCenter} cx={cx} cy={cy} r={11} />
        <text className={styles.rgClabel} x={cx} y={cy + 30} textAnchor="middle">
          {t('agent.graph.center')}
        </text>
        <g>{dots}</g>
        <g>{hubs}</g>
      </svg>
      <div className={styles.rcFoot}>
        <span className={styles.rcRun}>{t('agent.graph.running')}</span>
        <span>· {di} ●</span>
      </div>
    </div>
  );
}

// ── Правый dock: Отчёт (реальные данные из `final`) ──────────────────────────────────────────────

function ReportPane({ report, onToNote }: { report: string | null; onToNote: () => void }) {
  const { t } = useTranslation();
  if (!report) return <div className={styles.reportEmpty}>{t('agent.report.empty')}</div>;
  return (
    <div className={styles.report}>
      <div className={styles.repDoc}>
        <p className={styles.repP}>{report}</p>
      </div>
      <div className={styles.repActs}>
        <button type="button" className={styles.artBtn} onClick={onToNote}>
          <FilePlus2 size={14} aria-hidden /> {t('agent.report.toNote')}
        </button>
      </div>
    </div>
  );
}
