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
//
// P0-2 (полнота юниона): мок обязан уметь эмитить КАЖДЫЙ вариант `AgentStreamEvent` (гейт —
// `parity.test.ts`). Редкие варианты включаются триггерами по тексту задачи (как web/grounded-флаги
// у streamChat), дефолтный сценарий не меняется. Маркеры УЗКИЕ (анти-футган для смоуков):
// - слово «exec» (границы слова; «execute» НЕ триггерит) → пара `execProposal`→`execResult`
//   (SANDBOX-6c, после плана);
// - «report/отчёт»                → `report` (RES-5, после фазы changeset, перед final);
// - «демо-ошибка»/«demo-error»    → терминальный `error` ВМЕСТО final (провайдер упал);
//   обычные слова «ошибка/error» в задаче мок НЕ роняют.

import type {
  AgentApprovalDecision,
  AgentAutonomy,
  AgentHistoryMsg,
  AgentProposedKind,
  AgentSessionData,
  AgentSessionInfo,
  AgentStreamEvent,
  SkillList,
} from '../tauri-api';

/** Файлы демо-changeset'а (зеркало `proposal.files` контракта). `actionId` — синтетический адрес
 *  решения (как id строки `agent_actions`); approve адресует именно его. `kind` — `file` (правка/
 *  создание) | `exec` (командная строка, ACP-EXEC: рисуется как `$ cmd`). */
interface MockFile {
  path: string;
  add: number;
  del: number;
  status: 'new' | 'edit';
  kind: AgentProposedKind;
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
  // W-4: история мультитёрна — мок поведением её не использует (proposal детерминирован per-run),
  // но принимает для соответствия контракту команды `agent_run`.
  history: AgentHistoryMsg[] = [],
  // W-38: id переписки — мок принимает по контракту (персиста в браузер-превью нет).
  sessionId?: string,
): Promise<number> {
  void history; // принимаем по контракту; детерминированный мок-proposal от истории не зависит
  void sessionId; // W-38: браузер-мок не персистит историю (фейковый список ниже)
  // P0-2: триггеры редких вариантов юниона по тексту задачи (дефолтный прогон не меняется).
  // Узкие маркеры — анти-футган для Playwright-смоука: «execute…» НЕ триггерит exec (границы
  // слова), а легитимная задача про ошибки («разбери ошибку X») НЕ роняет мок — терминальный
  // error только по явному демо-маркеру «демо-ошибка»/«demo-error».
  const wantExec = /\bexec\b/i.test(task);
  const wantReport = /report|отч[её]т/i.test(task);
  const wantError = /демо-ошибка|demo-error/i.test(task);
  const runId = nextRunId++;
  const files: MockFile[] = [
    { path: 'RMS-B2B/Идея — кэш контекста.md', add: 8, del: 0, status: 'new', kind: 'file', actionId: runId * 100 + 1 },
    { path: 'RMS-B2B/00 — Карта проекта.md', add: 2, del: 1, status: 'edit', kind: 'file', actionId: runId * 100 + 2 },
    { path: 'PaymentService/Inbox-2.md', add: 5, del: 0, status: 'new', kind: 'file', actionId: runId * 100 + 3 },
    // ACP-EXEC: exec-permission (внешний ACP-агент) — командная строка, без ±строк/диффа.
    { path: 'git status --short', add: 0, del: 0, status: 'edit', kind: 'exec', actionId: runId * 100 + 4 },
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

      // P0-2: терминальная ошибка хода (зеркало `AgentStreamEvent::Error` — провайдер упал / бюджет
      // инициации стрима исчерпан). Как в реальном цикле: error ТЕРМИНАЛЕН — final не приходит.
      if (wantError) {
        onEvent({ type: 'error', message: 'мок: chat-провайдер недоступен (терминальная ошибка хода)' });
        return;
      }

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

      // W-15: note.create-вызов несёт {path, content} → фронт строит inline-дифф changeset'а (зеркало
      // реального бэка: предложения файлов РОЖДАЮТСЯ из note.create/edit tool-вызовов). Путь совпадает
      // с первым файлом proposal, чтобы дифф был доступен в превью.
      const writeId = 'mock-w1';
      onEvent({
        type: 'toolCall',
        id: writeId,
        kind: 'note.create',
        args: JSON.stringify({
          path: files[0].path,
          content: '# Идея — кэш контекста\n\nКэшировать контекст агентов между ходами.\n',
        }),
      });
      await sleep(STEP_MS * 2);
      onEvent({ type: 'toolResult', id: writeId, content: 'proposed', isError: false });

      // W-23: план/субагенты (SUB-2) — отдельные поля хода (рендер в W-24/25). Мок зеркалит контракт:
      // planProposed (шаги) → planStepStatus (прогресс по id) → subagentStatus (узел делегирования).
      if (run.cancelled) return;
      onEvent({
        type: 'planProposed',
        runId,
        steps: [
          { id: 's1', label: 'Прочитать Inbox', status: 'done' },
          { id: 's2', label: 'Создать заметки', status: 'running' },
          { id: 's3', label: 'Связать с проектами', status: 'pending' },
        ],
      });
      await sleep(STEP_MS);
      onEvent({ type: 'planStepStatus', id: 's2', status: 'done' });
      onEvent({ type: 'planStepStatus', id: 's3', status: 'running' });
      onEvent({
        type: 'subagentStatus',
        parentRunId: runId,
        childRunId: runId * 1000 + 1,
        goal: 'Сводка по проекту RMS-B2B',
        status: 'done',
        summary: 'Готово: 4 заметки, 2 связи.',
      });
      await sleep(STEP_MS);

      // P0-2 (SANDBOX-6c): exec-пара песочницы — `execProposal` (редакция-безопасный СИЛУЭТ: имя
      // инструмента + счётчики, БЕЗ сырых argv/env — приватность §5.6) → `execResult` (exit-код +
      // finalized, БЕЗ stdout/stderr). Формы — байт-в-байт зеркало Rust wire (`runId`/`actionId`/
      // `exitCode` — явный camelCase). Семантика: exec НИКОГДА не Auto — в реале между парой ВСЕГДА
      // стоит `decision_source.decide()`. Пара мока моделирует exec, УЖЕ ОДОБРЕННЫЙ host-side у
      // connected/ACP-бэкенда: в десктоп exec-события только ПРИХОДЯТ, решение выносится на стороне
      // хоста агента — десктоп для exec-решений зритель, UI-аппрува для exec нет НАМЕРЕННО (W-26).
      // Порядок как в реальном цикле: exec-вызов идёт ВНУТРИ хода (после плана), до end-of-turn
      // changeset. Пауза (kill-switch) подавляет «исполнение» между proposal и result.
      if (wantExec) {
        const execActionId = runId * 100 + 50;
        onEvent({
          type: 'execProposal',
          runId,
          actionId: execActionId,
          summary: 'shell.run · 2 args',
        });
        await sleep(STEP_MS * 2);
        await waitWhilePaused(run);
        if (run.cancelled) return;
        onEvent({
          type: 'execResult',
          runId,
          actionId: execActionId,
          exitCode: 0,
          finalized: true,
        });
        await sleep(STEP_MS);
      }

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
            kind: f.kind,
            actionId: f.actionId,
          })),
        });
        // Дифы по каждому ФАЙЛУ (эмитятся после proposal, по одному — как реальный гейт). Exec-строки
        // диффов не имеют (реальный бэк шлёт по ним execProposal/execResult, не diff) — пропускаем.
        for (const f of files) {
          if (run.cancelled) return;
          if (f.kind === 'exec') continue;
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
        // applied = одобренные ФАЙЛЫ (exec не «файл на диске» — у него нет undo-леджера).
        run.applied = files.filter((f) => f.kind !== 'exec' && decisions.get(f.actionId)).length;
      } else {
        // Auto-тир: применяется без аппрува (proposal НЕ эмитится — как гейт под autonomy=auto).
        // Exec-строки диффов не имеют — пропускаем (как реальный бэк: для них execProposal/execResult).
        for (const f of files) {
          if (run.cancelled) return;
          if (f.kind === 'exec') continue;
          await sleep(STEP_MS);
          onEvent({ type: 'diff', path: f.path, add: f.add, del: f.del, status: f.status });
        }
        run.applied = files.filter((f) => f.kind !== 'exec').length;
      }

      await waitWhilePaused(run);
      if (run.cancelled) return;

      // P0-2 (RES-5): отчёт deep-research — карточка дока. В реале эмитится ПОСЛЕ успешной записи
      // заметки отчёта через гейт → в моке после фазы changeset, ближе к финалу. Форма — зеркало
      // Rust wire (`runId`/`sourcesCount` — явный camelCase).
      if (wantReport) {
        onEvent({
          type: 'report',
          runId,
          title: 'Кэш контекста агентов — сводка',
          path: 'Research/Кэш контекста агентов.md',
          sourcesCount: 12,
          rounds: 3,
        });
        await sleep(STEP_MS);
      }

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

// W-10: SL-панель в браузер-превью/тестах. Один agent-навык + один vendor — для рендера UI.
const mockSkills = new Map<string, { pinned: boolean; archived: boolean }>([
  ['summarize-pr', { pinned: false, archived: false }],
]);
export async function listSkills(): Promise<SkillList> {
  return {
    learningEnabled: true,
    skillsDir: '.nexus/skills',
    parseErrors: 0,
    skills: [
      {
        name: 'summarize-pr',
        description: 'Кратко резюмирует diff пул-реквеста',
        tier: 'local',
        relPath: 'summarize-pr/SKILL.md',
        isVendor: false,
        useCount: 4,
        lastUsedAt: 1781170000,
        createdBy: 'agent',
        isAgentCreated: true,
        pinned: mockSkills.get('summarize-pr')!.pinned,
        state: mockSkills.get('summarize-pr')!.archived ? 'archived' : 'active',
        license: null,
      },
      {
        name: 'obsidian-markdown',
        description: 'Конвенции Obsidian-Markdown (вендоренный kepano)',
        tier: 'vendor',
        relPath: 'vendor/kepano/obsidian-markdown/SKILL.md',
        isVendor: true,
        useCount: 0,
        lastUsedAt: null,
        createdBy: 'vendor',
        isAgentCreated: false,
        pinned: false,
        state: null,
        license: 'MIT',
      },
    ],
  };
}
export async function setSkillPinned(name: string, pinned: boolean): Promise<boolean> {
  const s = mockSkills.get(name);
  if (!s) return false;
  s.pinned = pinned;
  return true;
}
export async function setSkillArchived(name: string, archived: boolean): Promise<boolean> {
  const s = mockSkills.get(name);
  if (!s) return false;
  s.archived = archived;
  return true;
}

// W-38: история переписок агента в браузер-превью/тестах. Несколько фейковых сессий (свежие сверху) +
// загрузка ходов по ним. Зеркалит контракт `agent_sessions_list`/`agent_session_load`.
const NOW = Math.floor(Date.now() / 1000);
const mockSessions: AgentSessionInfo[] = [
  {
    sessionId: 'sess-demo-1',
    title: 'Разобрать входящие заметки',
    status: 'done',
    turnCount: 2,
    updatedAt: NOW - 1800,
  },
  {
    sessionId: 'sess-demo-2',
    title: 'Связать проекты RMS-B2B',
    status: 'error',
    turnCount: 1,
    updatedAt: NOW - 86_400,
  },
  {
    sessionId: 'sess-demo-3',
    title: 'Сводка по PaymentService',
    status: 'done',
    turnCount: 3,
    updatedAt: NOW - 3 * 86_400,
  },
];

export async function sessionsList(): Promise<AgentSessionInfo[]> {
  return mockSessions.map((s) => ({ ...s }));
}

export async function sessionLoad(sessionId: string): Promise<AgentSessionData> {
  const info = mockSessions.find((s) => s.sessionId === sessionId);
  if (!info) return { turns: [] };
  // По одному ходу на turnCount — детерминированно, с одним tool-шагом, для рендера ленты.
  const turns = Array.from({ length: info.turnCount }, (_, i) => ({
    runId: 1000 + i,
    task: i === 0 ? info.title : `Уточнение ${i + 1}`,
    assistantText: `Готово по шагу ${i + 1}.`,
    report: i === info.turnCount - 1 ? `Итог переписки «${info.title}».` : null,
    error: null,
    status: 'done',
    createdAt: info.updatedAt - (info.turnCount - i) * 60,
    steps: [
      {
        kind: 'fs.read',
        args: '{"path":"Inbox.md"}',
        title: null,
        result: '12 записей',
        isError: false,
      },
    ],
  }));
  return { turns };
}
