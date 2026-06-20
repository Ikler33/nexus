// Браузер-мок агента (UI-1) — вне Tauri (превью/тесты). ОБЯЗАН зеркалить КОНТРАКТ команд `agent_*`
// (`Channel<AgentStreamEvent>` + run/approve/pause/resume/cancel/undo) ТОЧНО: те же формы событий,
// тот же порядок эмиссии, та же семантика approve (урок mock-must-match-backend — мок, который врёт,
// заставляет превью/тесты подтверждать неверное поведение).
//
// Контракт-зеркало (Rust `commands::agent`):
// - `run(task, autonomy, onEvent)` → Promise<run_id>; стрим: assistantToken… → toolCall → toolResult →
//   contextUsage → (confirm: proposal{runId,files[{…,actionId}]} → diff×N → final | auto: diff×N → final).
//   В `autonomy='confirm'` мок ждёт `approve(runId, decisions)` ПЕРЕД final (как гейт ждёт DecisionSource).
//   В `autonomy='auto'` файлы «применяются» без ожидания аппрува (Auto-тир), proposal не эмитится.
// - `approve(runId, decisions)` — кормит ожидающий прогон решениями (как `agent_approve` → UiDecisionSource).
// - `pause/resume` — тоггл паузы (стрим замирает/продолжается). `cancel` — обрывает стрим (как cancel-флаг).
// - `undo(runId)` — число «откаченных» действий (мок: количество применённых файлов прогона).

import type { AgentApprovalDecision, AgentAutonomy, AgentStreamEvent } from '../tauri-api';

/** Файлы демо-changeset'а (зеркало `proposal.files` контракта). `actionId` — синтетический адрес
 *  решения (как id строки `agent_actions`); approve адресует именно его. */
interface MockFile {
  path: string;
  add: number;
  del: number;
  status: 'new' | 'edit';
  actionId: number;
}

/** Состояние активного мок-прогона: gate-промис (ждёт approve в confirm-режиме), пауза, отмена. */
interface MockRun {
  autonomy: AgentAutonomy;
  files: MockFile[];
  /** Решения, присланные approve (actionId → approve). Confirm-гейт ждёт, пока они появятся. */
  decisions: Map<number, boolean> | null;
  /** Сигнал «решения пришли» — резолвит ожидание гейта (как `mpsc::recv` в UiDecisionSource). */
  resolveDecisions: ((d: Map<number, boolean>) => void) | null;
  paused: boolean;
  cancelled: boolean;
  /** Сколько файлов реально «применено» (для undo). */
  applied: number;
}

const runs = new Map<number, MockRun>();
let nextRunId = 1;

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const STEP_MS = 8;

/** Ждёт снятия паузы (как цикл проверяет kill-switch между ходами). Прерывается отменой. */
async function waitWhilePaused(run: MockRun): Promise<void> {
  while (run.paused && !run.cancelled) await sleep(STEP_MS);
}

/** Эмитит контент модели по словам как `assistantToken`-дельты (зеркало стрима токенов). */
async function streamTokens(
  run: MockRun,
  text: string,
  onEvent: (e: AgentStreamEvent) => void,
): Promise<boolean> {
  for (const tok of text.split(/(\s+)/)) {
    if (run.cancelled) return false;
    await waitWhilePaused(run);
    if (run.cancelled) return false;
    await sleep(STEP_MS);
    onEvent({ type: 'assistantToken', text: tok });
  }
  return true;
}

/**
 * Запускает мок-прогон. Возвращает Promise<run_id> СРАЗУ (как `agent_run` — run_id до завершения),
 * стрим докапывается асинхронно в `onEvent`. Зеркалит порядок реального цикла.
 */
export function run(
  task: string,
  autonomy: AgentAutonomy,
  onEvent: (event: AgentStreamEvent) => void,
): Promise<number> {
  const runId = nextRunId++;
  const files: MockFile[] = [
    { path: 'RMS-B2B/Идея — кэш контекста.md', add: 8, del: 0, status: 'new', actionId: runId * 100 + 1 },
    { path: 'RMS-B2B/00 — Карта проекта.md', add: 2, del: 1, status: 'edit', actionId: runId * 100 + 2 },
    { path: 'PaymentService/Inbox-2.md', add: 5, del: 0, status: 'new', actionId: runId * 100 + 3 },
  ];
  const run: MockRun = {
    autonomy,
    files,
    decisions: null,
    resolveDecisions: null,
    paused: false,
    cancelled: false,
    applied: 0,
  };
  runs.set(runId, run);

  void (async () => {
    try {
      // 1. Ассистент отвечает планом (стрим токенов). Текст короткий — стрим снапается быстро (превью/тест).
      const ok1 = await streamTokens(run, `Принял задачу: ${task}. План на 3 шага.`, onEvent);
      if (!ok1) return;

      // 2. Вызов инструмента ДО исполнения (`toolCall`) → результат (`toolResult`), корреляция по id.
      const callId = 'mock-c1';
      onEvent({ type: 'toolCall', id: callId, kind: 'fs.read', args: '{"path":"Inbox.md"}' });
      await waitWhilePaused(run);
      if (run.cancelled) return;
      await sleep(STEP_MS * 3);
      onEvent({
        type: 'toolResult',
        id: callId,
        content: '12 записей · 1.2 КБ\n— Идея: кэш контекста для агентов\n— Глянуть статью про RAG',
        isError: false,
      });

      // 3. Загрузка контекстного окна (`contextUsage`) — питает %-бар шапки.
      onEvent({ type: 'contextUsage', used: 24_000, window: 64_000 });
      await sleep(STEP_MS);

      // 4. Changeset. confirm: proposal (ждём approve) → diff×N → final. auto: diff×N (авто-apply) → final.
      if (run.autonomy === 'confirm') {
        onEvent({
          type: 'proposal',
          runId,
          files: files.map((f) => ({
            path: f.path,
            add: f.add,
            del: f.del,
            status: f.status,
            actionId: f.actionId,
          })),
        });
        // Дифы по каждому файлу (эмитятся после proposal, по одному — как реальный гейт).
        for (const f of files) {
          if (run.cancelled) return;
          await sleep(STEP_MS);
          onEvent({ type: 'diff', path: f.path, add: f.add, del: f.del, status: f.status });
        }
        // Гейт БЛОКИРУЕТСЯ на решении (как UiDecisionSource.decide ждёт agent_approve). Без approve
        // — fail-closed (ничего не применено); с approve — применяем одобренные.
        const decisions = await new Promise<Map<number, boolean>>((resolve) => {
          if (run.decisions) {
            resolve(run.decisions);
          } else {
            run.resolveDecisions = resolve;
          }
        });
        if (run.cancelled) return;
        run.applied = files.filter((f) => decisions.get(f.actionId)).length;
      } else {
        // Auto-тир: применяется без аппрува (proposal НЕ эмитится — как гейт под autonomy=auto).
        for (const f of files) {
          if (run.cancelled) return;
          await sleep(STEP_MS);
          onEvent({ type: 'diff', path: f.path, add: f.add, del: f.del, status: f.status });
        }
        run.applied = files.length;
      }

      await waitWhilePaused(run);
      if (run.cancelled) return;
      // 5. Финал хода.
      onEvent({
        type: 'final',
        text: 'Готово. Создал 3 заметки и связал их с проектами RMS-B2B и PaymentService.',
      });
    } finally {
      // Прогон не снимаем сразу из реестра: undo/approve могут прийти после final (как в реале строка
      // живёт). Чистим только при cancel — там стрим оборван намеренно. (Реестр мока — память процесса.)
    }
  })();

  return Promise.resolve(runId);
}

/** Кормит ожидающий confirm-прогон решениями (зеркало `agent_approve` → UiDecisionSource.decide). */
export function approve(runId: number, decisions: AgentApprovalDecision[]): Promise<void> {
  const run = runs.get(runId);
  if (!run) return Promise.reject(new Error(`agent_approve: прогон ${runId} не активен`));
  const map = new Map(decisions.map((d) => [d.actionId, d.approve]));
  run.decisions = map;
  // Разбудить гейт, если он уже ждёт (decide() висит на recv).
  run.resolveDecisions?.(map);
  run.resolveDecisions = null;
  return Promise.resolve();
}

export function pause(runId: number): Promise<void> {
  const run = runs.get(runId);
  if (!run) return Promise.reject(new Error(`agent_pause: прогон ${runId} не активен`));
  run.paused = true;
  return Promise.resolve();
}

export function resume(runId: number): Promise<void> {
  const run = runs.get(runId);
  if (!run) return Promise.reject(new Error(`agent_resume: прогон ${runId} не активен`));
  run.paused = false;
  return Promise.resolve();
}

export function cancel(runId: number): Promise<void> {
  const run = runs.get(runId);
  if (!run) return Promise.reject(new Error(`agent_cancel: прогон ${runId} не активен`));
  run.cancelled = true;
  run.paused = false;
  // Разбудить гейт, если он ждал approve — стрим должен корректно завершиться отменой.
  run.resolveDecisions?.(run.decisions ?? new Map());
  run.resolveDecisions = null;
  return Promise.resolve();
}

/** Число «откаченных» действий (мок: применённые файлы прогона; зеркало `agent_undo`). Идемпотентно. */
export function undo(runId: number): Promise<number> {
  const run = runs.get(runId);
  if (!run) return Promise.resolve(0);
  const restored = run.applied;
  run.applied = 0;
  return Promise.resolve(restored);
}

/** Тест-хелпер: сброс реестра прогонов между тестами (мок-бэкенд — память процесса). Помечает
 *  активные прогоны отменёнными (их async-петли остановятся) и НЕ сбрасывает `nextRunId` —
 *  монотонные id гарантируют, что осиротевший стрим прошлого теста не совпадёт run_id'ом с новым
 *  (иначе его поздние события прошли бы epoch-гард стора и протекли в следующий тест). */
export function __reset(): void {
  for (const run of runs.values()) {
    run.cancelled = true;
    run.resolveDecisions?.(new Map());
    run.resolveDecisions = null;
  }
  runs.clear();
}
