import { create } from 'zustand';

import { logUi } from '../lib/debug-log';
import { tauriApi } from '../lib/tauri-api';

import type {
  AgentApprovalDecision,
  AgentAutonomy,
  AgentFileStatus,
  AgentHistoryMsg,
  AgentPlanStep,
  AgentProposedKind,
  AgentStreamEvent,
  AgentSubagentState,
} from '../lib/tauri-api';

/**
 * Состояние вкладки Агента (UI-1b). Сессия = ОДНА задача + МУЛЬТИТЁРН внутри: каждое сообщение
 * пользователя пушит новый ХОД (`AgentTurn`) в ленту `turns`, а не стирает прошлое (фикс «переписка
 * исчезла на 2-м сообщении», 2026-06-23). Поток событий `AgentStreamEvent` (Tauri `Channel` или мок)
 * аккумулируется в АКТИВНЫЙ (последний) ход: текст ассистента (склейка `assistantToken`), шаги
 * (`toolCall`/`toolResult` по `id`), changeset (`proposal`/`diff` + per-file решение), отчёт (`final`),
 * ошибка (`error`). Загрузка контекста (`contextUsage`) — на уровне сессии (питает %-бар шапки).
 *
 * Один активный ход за раз (бэкенд держит реестр по run_id). `run()` — no-op, пока активный ход идёт.
 * autonomy/perms — политика сессии (читаются в момент `run`, на лету не меняют идущий ход).
 * «Новая сессия» (`newSession`) очищает ленту. Персист истории между запусками — отдельный срез.
 */

/** Статус хода. `awaiting` — changeset предложен, агент ждёт решения (Confirm-тир). */
export type AgentStatus =
  | 'idle'
  | 'running'
  | 'paused'
  | 'awaiting'
  | 'done'
  | 'error'
  | 'cancelled';

/** Шаг ленты = вызов инструмента + (опционально) его результат. Коррелируются по `id` из стрима. */
export interface AgentStep {
  id: string;
  kind: string;
  args: string;
  /** Человеко-подпись действия от агента (ACP `tool_call.title`, напр. «Fetching docs.rs»), если
   *  прислана. Отсутствует/`null` для нативного Кастора — `describeStep` строит подпись из
   *  `kind`+`args`. Опционально: фикстуры/нативный путь его не задают. */
  title?: string | null;
  /** Результат (`toolResult`) — приходит позже, по тому же `id`. `null` пока инструмент выполняется. */
  result: string | null;
  isError: boolean;
}

/** Решение пользователя по файлу changeset'а. `undefined` — ещё не решено (на ревью). */
export type FileDecision = 'applied' | 'rejected' | undefined;

/** Файл changeset'а в рендер-модели: контракт `proposal.files[]` + локальное решение. `kind` —
 *  `file` (путь + ±строки + дифф) | `exec` (командная строка `$ cmd`, без ±/диффа), ACP-EXEC. */
export interface ChangesetFile {
  path: string;
  add: number;
  del: number;
  status: AgentFileStatus;
  kind: AgentProposedKind;
  actionId: number;
  decision: FileDecision;
}

/** Загрузка контекстного окна (из `contextUsage`) — питает %-бар шапки. */
export interface ContextUsage {
  used: number;
  window: number;
}

/** Узел дерева делегирования (из `subagentStatus`; upsert по `childRunId`). Рендер — W-24. */
export interface SubagentNode {
  childRunId: number;
  parentRunId: number;
  goal: string;
  status: AgentSubagentState;
  /** Редакция-безопасный итог ребёнка (опускается, пока не пришёл). */
  summary?: string;
}

/** Exec-предложение песочницы (из `execProposal`; `execResult` дополняет exit-код). Рендер — W-26.
 *  `summary` — силуэт (имя инструмента + счётчики), БЕЗ сырых argv/env. `exitCode`/`finalized` —
 *  `null`/`false`, пока exec не завершён (приватность §5.6: сырого stdout тут нет by-design). */
export interface ExecItem {
  runId: number;
  actionId: number;
  summary: string;
  exitCode: number | null;
  finalized: boolean;
}

/** Отчёт deep-research (из `report`) — карточка дока. Рендер — W-25. */
export interface ResearchReportDoc {
  runId: number;
  title: string;
  path: string;
  sourcesCount: number;
  rounds: number;
}

/** Права в хранилище (per-run политика; зеркало макета `perms`). Read/Write/Web — для шапки/настроек. */
export interface AgentPerms {
  read: boolean;
  write: boolean;
  web: boolean;
}

/** Один ход диалога с агентом: задача пользователя + аккумулированный ответ/действия агента. */
export interface AgentTurn {
  /** Локальный стабильный ключ хода (react-key; растёт в пределах сессии, сбрасывается newSession). */
  key: number;
  /** Монотонный epoch-токен прогона (НЕ сбрасывается newSession). Гард событий: late-событие
   *  прошлого прогона не попадёт в новый ход даже при совпадении `key` (после newSession) или до
   *  прихода backend-`runId`. Закрывает окно «события прошлого прогона текут в новый». */
  epoch: number;
  /** id прогона на бэкенде (`null`, пока `agent_run` не вернул id). */
  runId: number | null;
  /** Сообщение пользователя, начавшее этот ход (для первого хода — «Задача сессии»). */
  task: string;
  /** Склеенный контент ассистента (`assistantToken`-дельты). */
  assistantText: string;
  /** Лента шагов (tool-вызовы + результаты). */
  steps: AgentStep[];
  /** Файлы changeset'а (из `proposal`; `diff` дополняет счётчики). */
  changeset: ChangesetFile[];
  /** План прогона (из `planProposed`; `planStepStatus` обновляет статусы по `id`). Рендер — W-24/25. */
  plan: AgentPlanStep[];
  /** Дерево субагентов (из `subagentStatus`, upsert по `childRunId`). Рендер — W-24. */
  subagents: SubagentNode[];
  /** Exec-предложения песочницы (из `execProposal`/`execResult`, по `actionId`). Рендер — W-26. */
  execItems: ExecItem[];
  /** Отчёт deep-research (из `report`). Рендер — W-25. */
  researchReport: ResearchReportDoc | null;
  /** Итоговый ответ (`final`). */
  report: string | null;
  /** Текст ошибки (`error`-событие / сбой `agent_run`). */
  error: string | null;
  status: AgentStatus;
}

interface AgentState {
  /** Лента ходов сессии (мультитёрн). Пусто — сессия ещё не начата. */
  turns: AgentTurn[];
  /** W-38: id текущей переписки (история). Генерится при инициализации и в `newSession`; едет в
   *  `agent_run` как group-ключ персиста ходов. Меняется на загруженный при `loadSession`. */
  currentSessionId: string;
  autonomy: AgentAutonomy;
  // B10: стейта «модель» здесь больше НЕТ — селектор был фикцией (реальную модель выбирает бэкенд
  // по конфигу, значение никуда не передавалось). Вернём при INFER-CFG (per-run выбор на бэкенде).
  perms: AgentPerms;
  context: ContextUsage | null;
  /** Идёт ли отправка решений в `agent_approve` (блок кнопок аппрува). */
  approving: boolean;

  /** Запускает ход по задаче (читает текущие autonomy/perms). No-op во время активного хода. */
  run: (task: string) => void;
  setAutonomy: (autonomy: AgentAutonomy) => void;
  setPerm: (key: keyof AgentPerms, value: boolean) => void;
  /** Поставить решение по файлу активного хода (повтор того же решения снимает — как тоггл макета). */
  setFileDecision: (actionId: number, decision: 'applied' | 'rejected') => void;
  /** Массовое решение по всем файлам активного хода (bulk apply-all / reject). */
  setAllDecisions: (decision: 'applied' | 'rejected') => void;
  /** Собирает `decisions[]` из per-file решений активного хода и шлёт `agent_approve`. Нерешённые
   *  файлы по умолчанию = reject (fail-closed, как бэкенд: отсутствующий айтем = Reject). */
  approve: () => Promise<void>;
  cancel: () => Promise<void>;
  /** Откат применённых действий активного/последнего хода (AGENT-4) → число откаченных. */
  undo: () => Promise<number>;
  /** W-38: переоткрывает прошлую переписку — грузит её ходы (`agent_session_load`) в ленту и делает её
   *  текущей (нельзя во время активного хода). */
  loadSession: (sessionId: string) => Promise<void>;
  /** Новая сессия: очищает ленту (нельзя во время активного хода — сначала cancel). */
  newSession: () => void;
}

/** Терминальные статусы — ход завершён, можно стартовать новый / аппрув уже не нужен. */
const TERMINAL: AgentStatus[] = ['idle', 'done', 'error', 'cancelled'];

/** Активен ли ход (стрим идёт / на паузе / ждёт аппрува). */
function isActive(status: AgentStatus): boolean {
  return !TERMINAL.includes(status);
}

/** Статус сессии = статус последнего хода (или `idle`, если ходов нет). Для шапки/композера. */
export function sessionStatus(turns: AgentTurn[]): AgentStatus {
  return turns.length ? turns[turns.length - 1].status : 'idle';
}

/** W-38: новый id переписки — `sess-<ms>-<rand>` (уникален в пределах запуска для группировки ходов). */
function newSessionId(): string {
  return `sess-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

const INITIAL = {
  turns: [] as AgentTurn[],
  context: null as ContextUsage | null,
  approving: false,
};

/** Монотонный счётчик epoch прогонов (память модуля; НЕ сбрасывается newSession — в отличие от `key`).
 *  Каждый `run()` берёт уникальный epoch → события строго адресуются своему ходу. */
let agentEpochSeq = 0;

export const useAgentStore = create<AgentState>((set, get) => ({
  ...INITIAL,
  currentSessionId: newSessionId(),
  autonomy: 'confirm',
  perms: { read: true, write: true, web: false },

  run(task) {
    const q = task.trim();
    const last = get().turns[get().turns.length - 1];
    if (!q || (last && isActive(last.status))) return;
    const { autonomy } = get();
    logUi('agent:run', `autonomy=${autonomy} len=${q.length} turn=${get().turns.length}`);
    // Новый ХОД дописывается в ленту (НЕ стираем прошлые ходы — фикс стирания переписки).
    const turnKey = last ? last.key + 1 : 0;
    const myEpoch = ++agentEpochSeq;
    // W-4: история прошлых ходов → бэкенд (иначе follow-up не помнит контекст и не предлагает правки →
    // не было changeset-гейта, ST-G3). Берём до добавления нового хода. Защиты (ревью W-4):
    //  • КАЖДЫЙ ход даёт user+assistant (пустой ответ errored/cancelled → плейсхолдер) — строгая
    //    альтернация ролей (часть OpenAI-серверов 400-ит на двух подряд user/assistant);
    //  • кап по ходам И по символам (огромный отчёт иначе раздул бы контекст → hard-fail прогона);
    //  • усечение одного сообщения; набираем с КОНЦА (свежее важнее), всегда ≥ последний ход.
    const HISTORY_TURNS_CAP = 8;
    const HISTORY_CHAR_BUDGET = 12000;
    const PER_MSG_CHARS = 4000;
    const trunc = (s: string) => (s.length > PER_MSG_CHARS ? `${s.slice(0, PER_MSG_CHARS)}…` : s);
    const recent = get().turns.slice(-HISTORY_TURNS_CAP);
    const built: AgentHistoryMsg[] = [];
    let budget = HISTORY_CHAR_BUDGET;
    for (let i = recent.length - 1; i >= 0; i--) {
      const tn = recent[i];
      const user = trunc(tn.task);
      const answer = trunc((tn.report ?? tn.assistantText ?? '').trim() || '(нет ответа)');
      // Всегда оставляем хотя бы самый свежий ход; иначе обрезаем по бюджету.
      if (built.length > 0 && budget - (user.length + answer.length) < 0) break;
      budget -= user.length + answer.length;
      built.push({ role: 'assistant', text: answer }, { role: 'user', text: user });
    }
    const history: AgentHistoryMsg[] = built.reverse();
    set((s) => ({
      turns: [
        ...s.turns,
        {
          key: turnKey,
          epoch: myEpoch,
          runId: null,
          task: q,
          assistantText: '',
          steps: [],
          changeset: [],
          plan: [],
          subagents: [],
          execItems: [],
          researchReport: null,
          report: null,
          error: null,
          status: 'running' as AgentStatus,
        },
      ],
    }));

    /** Патч конкретного хода по ключу (события адресуются СВОЕМУ ходу, не «последнему»). */
    const patch = (fn: (tn: AgentTurn) => AgentTurn) =>
      set((s) => ({ turns: s.turns.map((tn) => (tn.key === turnKey ? fn(tn) : tn)) }));

    // Аккумулятор событий стрима → активный ход. Epoch-гард: событие применяется ТОЛЬКО к СВОЕМУ
    // ходу (по `epoch`, а не «последнему») — закрывает окно ДО прихода runId и реюз `key` после
    // newSession; late-события прошлого прогона в чужую ленту не текут.
    const onEvent = (event: AgentStreamEvent) => {
      const tn = get().turns.find((t) => t.key === turnKey);
      if (!tn || tn.epoch !== myEpoch) return;
      if (TERMINAL.includes(tn.status)) {
        // Ход уже завершён — поздние токены не принимаем (кроме штатных final/error)…
        if (event.type !== 'final' && event.type !== 'error') return;
        // …и НЕ воскрешаем ОТМЕНЁННЫЙ ход: cancel = явное намерение финала (уважаем решение юзера).
        if (tn.status === 'cancelled') return;
      }
      switch (event.type) {
        case 'assistantToken':
          patch((t0) => ({ ...t0, assistantText: t0.assistantText + event.text }));
          break;
        case 'toolCall':
          patch((t0) => ({
            ...t0,
            steps: [
              ...t0.steps,
              {
                id: event.id,
                kind: event.kind,
                args: event.args,
                title: event.title ?? null,
                result: null,
                isError: false,
              },
            ],
          }));
          break;
        case 'toolResult':
          patch((t0) => ({
            ...t0,
            steps: t0.steps.map((st) =>
              st.id === event.id ? { ...st, result: event.content, isError: event.isError } : st,
            ),
          }));
          break;
        case 'contextUsage':
          // Контекст — на уровне сессии (последнее значение питает %-бар шапки).
          set({ context: { used: event.used, window: event.window } });
          break;
        case 'proposal':
          // Changeset предложен → агент ждёт решения (Confirm-тир). Auto-режим proposal НЕ шлёт.
          patch((t0) => ({
            ...t0,
            changeset: event.files.map((f) => ({
              path: f.path,
              add: f.add,
              del: f.del,
              status: f.status,
              kind: f.kind,
              actionId: f.actionId,
              decision: undefined,
            })),
            status: t0.status === 'paused' ? 'paused' : 'awaiting',
          }));
          break;
        case 'diff':
          // Диф по файлу. Если файла нет в changeset (auto-режим без proposal) — заводим запись (без
          // actionId-аппрува: в auto он применяется агентом). Дедуп по path (proposal уже завёл).
          patch((t0) => {
            if (t0.changeset.some((f) => f.path === event.path)) return t0;
            return {
              ...t0,
              changeset: [
                ...t0.changeset,
                {
                  path: event.path,
                  add: event.add,
                  del: event.del,
                  status: event.status,
                  kind: 'file', // diff'ы — всегда файловые изменения (exec идёт через execProposal)
                  actionId: -1, // auto-diff без proposal: не адресуется аппрувом
                  decision: 'applied',
                },
              ],
            };
          });
          break;
        case 'planProposed':
          // Предложен план (SUB-2/RES) → док «План/Граф». Полностью заменяет (новый план хода).
          patch((t0) => ({ ...t0, plan: event.steps }));
          break;
        case 'planStepStatus':
          // Обновление статуса ОДНОГО шага плана по стабильному id.
          patch((t0) => ({
            ...t0,
            plan: t0.plan.map((s) => (s.id === event.id ? { ...s, status: event.status } : s)),
          }));
          break;
        case 'subagentStatus':
          // Узел дерева делегирования — upsert по childRunId (повторное событие обновляет статус/итог).
          patch((t0) => {
            const node: SubagentNode = {
              childRunId: event.childRunId,
              parentRunId: event.parentRunId,
              goal: event.goal,
              status: event.status,
              summary: event.summary,
            };
            const exists = t0.subagents.some((s) => s.childRunId === event.childRunId);
            return {
              ...t0,
              subagents: exists
                ? t0.subagents.map((s) => (s.childRunId === event.childRunId ? node : s))
                : [...t0.subagents, node],
            };
          });
          break;
        case 'execProposal':
          // Exec-предложение песочницы — заводим запись (по actionId), exit-код придёт в execResult.
          patch((t0) => {
            if (t0.execItems.some((e) => e.actionId === event.actionId)) return t0;
            return {
              ...t0,
              execItems: [
                ...t0.execItems,
                {
                  runId: event.runId,
                  actionId: event.actionId,
                  summary: event.summary,
                  exitCode: null,
                  finalized: false,
                },
              ],
            };
          });
          break;
        case 'execResult':
          // Результат exec по actionId: проставляем exit-код/finalized. Нет предложения (силуэт мог
          // не дойти) — заводим запись без summary, чтобы факт исполнения не потерялся.
          patch((t0) => {
            const exists = t0.execItems.some((e) => e.actionId === event.actionId);
            return {
              ...t0,
              execItems: exists
                ? t0.execItems.map((e) =>
                    e.actionId === event.actionId
                      ? { ...e, exitCode: event.exitCode, finalized: event.finalized }
                      : e,
                  )
                : [
                    ...t0.execItems,
                    {
                      runId: event.runId,
                      actionId: event.actionId,
                      summary: '',
                      exitCode: event.exitCode,
                      finalized: event.finalized,
                    },
                  ],
            };
          });
          break;
        case 'report':
          // Отчёт deep-research (RES-5) — карточка дока (рендер W-25).
          patch((t0) => ({
            ...t0,
            researchReport: {
              runId: event.runId,
              title: event.title,
              path: event.path,
              sourcesCount: event.sourcesCount,
              rounds: event.rounds,
            },
          }));
          break;
        case 'final':
          patch((t0) => ({ ...t0, report: event.text, status: 'done' }));
          break;
        case 'error':
          patch((t0) => ({ ...t0, error: event.message, status: 'error' }));
          break;
      }
    };

    void tauriApi.agent
      .run(q, autonomy, onEvent, history, get().currentSessionId)
      .then((id) => {
        const tn = get().turns.find((t) => t.key === turnKey);
        // Тот же ход (epoch), не отменён синхронным потоком до резолва id — иначе не привязываем runId.
        if (!tn || tn.epoch !== myEpoch || tn.status === 'cancelled') return;
        patch((t0) => ({ ...t0, runId: id }));
      })
      .catch(() => {
        // onEvent уже получил error-событие (tauri-api прокидывает) → статус выставлен. Здесь молча.
      });
  },

  setAutonomy(autonomy) {
    if (isActive(sessionStatus(get().turns))) return; // per-run политика — на лету не меняем
    set({ autonomy });
  },
  setPerm(key, value) {
    if (isActive(sessionStatus(get().turns))) return;
    set((s) => ({ perms: { ...s.perms, [key]: value } }));
  },

  setFileDecision(actionId, decision) {
    set((s) => ({
      turns: s.turns.map((tn, i) =>
        i === s.turns.length - 1
          ? {
              ...tn,
              changeset: tn.changeset.map((f) =>
                f.actionId === actionId
                  ? { ...f, decision: f.decision === decision ? undefined : decision }
                  : f,
              ),
            }
          : tn,
      ),
    }));
  },
  setAllDecisions(decision) {
    set((s) => ({
      turns: s.turns.map((tn, i) =>
        i === s.turns.length - 1
          ? { ...tn, changeset: tn.changeset.map((f) => ({ ...f, decision })) }
          : tn,
      ),
    }));
  },

  async approve() {
    const turns = get().turns;
    const last = turns[turns.length - 1];
    if (!last || last.runId == null || last.status !== 'awaiting' || get().approving) return;
    // `approve()` = УТВЕРДИТЕЛЬНАЯ кнопка «Подтвердить» (её единственный вызыватель). Поэтому одобряем
    // всё, что НЕ отклонено ЯВНО: `applied` И `undefined` → approve; только явный `rejected` → reject.
    // (Раньше `undefined → reject` ломало UX: клик «Подтвердить» без пометки строк отклонял permission.)
    // Fail-closed «ничего не подтвердили» остаётся на БЭКЕНДЕ: незавершённый ход → pending → Cancelled.
    // Только адресуемые файлы (actionId >= 0).
    //
    // ACP: один ACP-permission = ОДНО атомарное решение, поэтому N файлов делят ОДИН actionId.
    // Дедуплицируем по actionId и шлём ОДНО решение на группу. Семантика группы — AND по «не отклонено»:
    // любой ЯВНО отклонённый файл → reject всей атомарной permission (нельзя частично одобрить).
    // Для embedded (уникальные id) каждая группа = один файл.
    const byAction = new Map<number, boolean>();
    for (const f of last.changeset) {
      if (f.actionId < 0) continue;
      const ok = f.decision !== 'rejected';
      byAction.set(f.actionId, (byAction.get(f.actionId) ?? true) && ok);
    }
    const decisions: AgentApprovalDecision[] = [...byAction.entries()].map(
      ([actionId, approve]) => ({ actionId, approve }),
    );
    if (!decisions.length) return;
    const runId = last.runId;
    const lastKey = last.key;
    logUi('agent:approve', `n=${decisions.length} ok=${decisions.filter((d) => d.approve).length}`);
    set({ approving: true });
    try {
      await tauriApi.agent.approve(runId, decisions);
      // Решение принято — нерешённые помечаем applied (отражаем то, что ушло на бэк: «Подтвердить»
      // одобряет всё не отклонённое явно), снимаем ожидание.
      set((s) => ({
        approving: false,
        turns: s.turns.map((tn) =>
          tn.key === lastKey
            ? {
                ...tn,
                status: tn.status === 'awaiting' ? 'running' : tn.status,
                changeset: tn.changeset.map((f) =>
                  f.actionId >= 0 && f.decision === undefined ? { ...f, decision: 'applied' } : f,
                ),
              }
            : tn,
        ),
      }));
    } catch {
      set({ approving: false });
    }
  },

  async cancel() {
    const last = get().turns[get().turns.length - 1];
    if (!last || last.runId == null || !isActive(last.status)) return;
    const lastKey = last.key;
    logUi('agent:cancel', `run=${last.runId}`);
    set((s) => ({
      turns: s.turns.map((tn) => (tn.key === lastKey ? { ...tn, status: 'cancelled' } : tn)),
    }));
    try {
      await tauriApi.agent.cancel(last.runId);
    } catch {
      /* уже не активен */
    }
  },
  async undo() {
    const last = get().turns[get().turns.length - 1];
    if (!last || last.runId == null) return 0;
    try {
      return await tauriApi.agent.undo(last.runId);
    } catch {
      return 0;
    }
  },

  async loadSession(sessionId) {
    if (isActive(sessionStatus(get().turns))) return; // активный ход сначала отменить
    logUi('agent:load-session', sessionId);
    let data;
    try {
      data = await tauriApi.agent.sessions.load(sessionId);
    } catch {
      return; // загрузка не удалась — ленту не трогаем
    }
    // Реконструируем ленту: по одному `AgentTurn` на персистированный ход. Live-only поля (план/
    // субагенты/exec/changeset/отчёт-документ) пусты — персист их не несёт (W-38), они оживают только в
    // НОВЫХ прогонах. `key`/`epoch` присваиваем возрастающе (epoch — из общего счётчика, чтобы
    // late-события активных прогонов сюда не текли).
    const turns: AgentTurn[] = data.turns.map((tn, i) => ({
      key: i,
      epoch: ++agentEpochSeq,
      runId: tn.runId,
      task: tn.task,
      assistantText: tn.assistantText,
      steps: tn.steps.map((s, j) => ({
        id: `hist-${tn.runId}-${j}`,
        kind: s.kind,
        args: s.args,
        title: s.title,
        result: s.result,
        isError: s.isError,
      })),
      changeset: [],
      plan: [],
      subagents: [],
      execItems: [],
      researchReport: null,
      report: tn.report,
      error: tn.error,
      status: tn.status as AgentStatus,
    }));
    set({ turns, currentSessionId: sessionId, context: null, approving: false });
  },

  newSession() {
    if (isActive(sessionStatus(get().turns))) return; // активный ход сначала отменить
    logUi('agent:new-session');
    set({ ...INITIAL, currentSessionId: newSessionId() });
  },
}));
