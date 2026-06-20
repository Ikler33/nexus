import { create } from 'zustand';

import { logUi } from '../lib/debug-log';
import { tauriApi } from '../lib/tauri-api';

import type {
  AgentApprovalDecision,
  AgentAutonomy,
  AgentFileStatus,
  AgentStreamEvent,
} from '../lib/tauri-api';

/**
 * Состояние вкладки Агента (UI-1b). Прогон = поток событий `AgentStreamEvent` через `Channel` (Tauri)
 * или мок (браузер), который `run()` копит в РЕНДЕР-МОДЕЛЬ: текст ассистента (склейка `assistantToken`),
 * ШАГИ (`toolCall`/`toolResult` по `id`), загрузка контекста (`contextUsage`), CHANGESET (`proposal`/
 * `diff` + per-file решение), отчёт (`final`), ошибка (`error`).
 *
 * Один активный прогон за раз (как бэкенд держит реестр по run_id). Аппрув собирает `decisions[]` из
 * per-file состояния changeset'а и шлёт `agent_approve`. autonomy/model/perms — per-run политика
 * (читаются в момент `run`, на лету не меняют идущий прогон).
 */

/** Статус прогона. `awaiting` — changeset предложен, агент ждёт решения (Confirm-тир). */
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
  /** Результат (`toolResult`) — приходит позже, по тому же `id`. `null` пока инструмент выполняется. */
  result: string | null;
  isError: boolean;
}

/** Решение пользователя по файлу changeset'а. `undefined` — ещё не решено (на ревью). */
export type FileDecision = 'applied' | 'rejected' | undefined;

/** Файл changeset'а в рендер-модели: контракт `proposal.files[]` + локальное решение. */
export interface ChangesetFile {
  path: string;
  add: number;
  del: number;
  status: AgentFileStatus;
  actionId: number;
  decision: FileDecision;
}

/** Загрузка контекстного окна (из `contextUsage`) — питает %-бар шапки. */
export interface ContextUsage {
  used: number;
  window: number;
}

/** Права в хранилище (per-run политика; зеркало макета `perms`). Read/Write/Web — для шапки/настроек. */
export interface AgentPerms {
  read: boolean;
  write: boolean;
  web: boolean;
}

interface AgentState {
  /** id текущего/последнего прогона (`null` — ещё не запускали). */
  runId: number | null;
  status: AgentStatus;
  /** Задача текущего прогона (промпт сессии). */
  task: string;
  autonomy: AgentAutonomy;
  /** Отображаемая модель (per-run политика UI; реальную выбирает бэкенд по конфигу). */
  model: string;
  perms: AgentPerms;
  /** Склеенный контент ассистента (`assistantToken`-дельты). */
  assistantText: string;
  /** Лента шагов (tool-вызовы + результаты). */
  steps: AgentStep[];
  context: ContextUsage | null;
  /** Файлы changeset'а (из `proposal`; `diff` дополняет счётчики). */
  changeset: ChangesetFile[];
  /** Идёт ли отправка решений в `agent_approve` (блок кнопок аппрува). */
  approving: boolean;
  /** Итоговый ответ (`final`) — питает отчёт правого дока. */
  report: string | null;
  /** Текст ошибки (`error`-событие / сбой `agent_run`). */
  error: string | null;

  /** Запускает прогон по задаче (читает текущие autonomy/model/perms). No-op во время активного прогона. */
  run: (task: string) => void;
  setAutonomy: (autonomy: AgentAutonomy) => void;
  setModel: (model: string) => void;
  setPerm: (key: keyof AgentPerms, value: boolean) => void;
  /** Поставить решение по файлу (повтор того же решения снимает — как тоггл макета). */
  setFileDecision: (actionId: number, decision: 'applied' | 'rejected') => void;
  /** Массовое решение по всем файлам (bulk apply-all / reject). */
  setAllDecisions: (decision: 'applied' | 'rejected') => void;
  /** Собирает `decisions[]` из per-file решений changeset'а и шлёт `agent_approve`. Нерешённые файлы
   *  по умолчанию считаются reject (fail-closed, как бэкенд: отсутствующий айтем = Reject). */
  approve: () => Promise<void>;
  pause: () => Promise<void>;
  resume: () => Promise<void>;
  cancel: () => Promise<void>;
  /** Откат применённых действий прогона (AGENT-4) → число откаченных. */
  undo: () => Promise<number>;
  /** Новая сессия: чистый прогон (нельзя во время активного — сначала cancel). */
  newSession: () => void;
}

/** Терминальные статусы — прогон завершён, можно стартовать новый / аппрув уже не нужен. */
const TERMINAL: AgentStatus[] = ['idle', 'done', 'error', 'cancelled'];

/** Активен ли прогон (стрим идёт / на паузе / ждёт аппрува). */
function isActive(status: AgentStatus): boolean {
  return !TERMINAL.includes(status);
}

const INITIAL = {
  runId: null,
  status: 'idle' as AgentStatus,
  task: '',
  assistantText: '',
  steps: [] as AgentStep[],
  context: null,
  changeset: [] as ChangesetFile[],
  approving: false,
  report: null,
  error: null,
};

export const useAgentStore = create<AgentState>((set, get) => ({
  ...INITIAL,
  autonomy: 'confirm',
  model: 'qwen3:35b',
  perms: { read: true, write: true, web: false },

  run(task) {
    const q = task.trim();
    if (!q || isActive(get().status)) return;
    const { autonomy } = get();
    logUi('agent:run', `autonomy=${autonomy} len=${q.length}`);
    // Новый прогон — чистим рендер-модель прошлого (task/autonomy/model/perms сохраняются).
    set({
      ...INITIAL,
      task: q,
      status: 'running',
    });

    // Аккумулятор событий стрима → рендер-модель. Epoch-гард по runId: поздние события прошлого
    // прогона (после cancel/нового run) игнорируем — иначе они дописались бы в чужую ленту.
    let myRunId: number | null = null;
    const onEvent = (event: AgentStreamEvent) => {
      // До прихода run_id принимаем события текущего прогона (status='running' выставлен синхронно).
      // После — только если runId совпадает с нашим прогоном.
      if (myRunId != null && get().runId !== myRunId) return;
      if (TERMINAL.includes(get().status) && get().status !== 'idle') {
        // Уже завершено (cancel/error) — поздние токены не принимаем (кроме штатного потока до final).
        if (event.type !== 'final' && event.type !== 'error') return;
      }
      switch (event.type) {
        case 'assistantToken':
          set((s) => ({ assistantText: s.assistantText + event.text }));
          break;
        case 'toolCall':
          set((s) => ({
            steps: [
              ...s.steps,
              { id: event.id, kind: event.kind, args: event.args, result: null, isError: false },
            ],
          }));
          break;
        case 'toolResult':
          set((s) => ({
            steps: s.steps.map((st) =>
              st.id === event.id ? { ...st, result: event.content, isError: event.isError } : st,
            ),
          }));
          break;
        case 'contextUsage':
          set({ context: { used: event.used, window: event.window } });
          break;
        case 'proposal':
          // Changeset предложен → агент ждёт решения (Confirm-тир). Auto-режим proposal НЕ шлёт.
          set({
            changeset: event.files.map((f) => ({
              path: f.path,
              add: f.add,
              del: f.del,
              status: f.status,
              actionId: f.actionId,
              decision: undefined,
            })),
            status: get().status === 'paused' ? 'paused' : 'awaiting',
          });
          break;
        case 'diff':
          // Диф по файлу. Если файла нет в changeset (auto-режим без proposal) — заводим запись (без
          // actionId-аппрува: в auto он применяется агентом). Дедуп по path (proposal уже завёл).
          set((s) => {
            if (s.changeset.some((f) => f.path === event.path)) return s;
            return {
              changeset: [
                ...s.changeset,
                {
                  path: event.path,
                  add: event.add,
                  del: event.del,
                  status: event.status,
                  actionId: -1, // auto-diff без proposal: не адресуется аппрувом
                  decision: 'applied',
                },
              ],
            };
          });
          break;
        case 'final':
          set({ report: event.text, status: 'done' });
          break;
        case 'error':
          set({ error: event.message, status: 'error' });
          break;
      }
    };

    void tauriApi.agent
      .run(q, autonomy, onEvent)
      .then((id) => {
        myRunId = id;
        // Прогон мог уже завершиться/отмениться синхронным потоком до резолва id — не воскрешаем.
        if (get().status === 'cancelled') return;
        set({ runId: id });
      })
      .catch(() => {
        // onEvent уже получил error-событие (tauri-api прокидывает) → статус выставлен. Здесь молча.
      });
  },

  setAutonomy(autonomy) {
    if (isActive(get().status)) return; // per-run политика — на лету не меняем
    set({ autonomy });
  },
  setModel(model) {
    if (isActive(get().status)) return;
    set({ model });
  },
  setPerm(key, value) {
    if (isActive(get().status)) return;
    set((s) => ({ perms: { ...s.perms, [key]: value } }));
  },

  setFileDecision(actionId, decision) {
    set((s) => ({
      changeset: s.changeset.map((f) =>
        f.actionId === actionId
          ? { ...f, decision: f.decision === decision ? undefined : decision }
          : f,
      ),
    }));
  },
  setAllDecisions(decision) {
    set((s) => ({ changeset: s.changeset.map((f) => ({ ...f, decision })) }));
  },

  async approve() {
    const { runId, changeset, status } = get();
    if (runId == null || status !== 'awaiting' || get().approving) return;
    // decisions[]: одобренные = applied; всё прочее (rejected / нерешённое) = reject (fail-closed,
    // как бэкенд трактует отсутствующий айтем). Только адресуемые файлы (actionId >= 0).
    const decisions: AgentApprovalDecision[] = changeset
      .filter((f) => f.actionId >= 0)
      .map((f) => ({ actionId: f.actionId, approve: f.decision === 'applied' }));
    if (!decisions.length) return;
    logUi('agent:approve', `n=${decisions.length} ok=${decisions.filter((d) => d.approve).length}`);
    set({ approving: true });
    try {
      await tauriApi.agent.approve(runId, decisions);
      // Решение принято — нерешённые помечаем reject (отражаем то, что ушло на бэк), снимаем ожидание.
      set((s) => ({
        approving: false,
        status: s.status === 'awaiting' ? 'running' : s.status,
        changeset: s.changeset.map((f) =>
          f.actionId >= 0 && f.decision === undefined ? { ...f, decision: 'rejected' } : f,
        ),
      }));
    } catch {
      set({ approving: false });
    }
  },

  async pause() {
    const { runId, status } = get();
    if (runId == null || (status !== 'running' && status !== 'awaiting')) return;
    try {
      await tauriApi.agent.pause(runId);
      set({ status: 'paused' });
    } catch {
      /* прогон не активен — статус не трогаем */
    }
  },
  async resume() {
    const { runId, status } = get();
    if (runId == null || status !== 'paused') return;
    try {
      await tauriApi.agent.resume(runId);
      // Возвращаемся в running (если ждали аппрув — changeset всё ещё на ревью, кнопки активны).
      set((s) => ({ status: s.changeset.some((f) => f.decision === undefined && f.actionId >= 0) ? 'awaiting' : 'running' }));
    } catch {
      /* no-op */
    }
  },
  async cancel() {
    const { runId } = get();
    if (runId == null || !isActive(get().status)) return;
    logUi('agent:cancel', `run=${runId}`);
    set({ status: 'cancelled' });
    try {
      await tauriApi.agent.cancel(runId);
    } catch {
      /* уже не активен */
    }
  },
  async undo() {
    const { runId } = get();
    if (runId == null) return 0;
    try {
      return await tauriApi.agent.undo(runId);
    } catch {
      return 0;
    }
  },

  newSession() {
    if (isActive(get().status)) return; // активный прогон сначала отменить
    logUi('agent:new-session');
    set({ ...INITIAL });
  },
}));
