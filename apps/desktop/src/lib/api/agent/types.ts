/**
 * DTO-типы agent-домена (F-2c): wire-события агент-стрима и их спутники, аппрув changeset'а,
 * история переписок (W-38), навыки SL-панели (W-10), подключение агента (CONN-4/ACP). Зеркала
 * Rust-структур (`commands::agent` / `agent::connect::wire` / `settings`) — контракт провода
 * `invoke`. Потребители импортируют их по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

// ── Агент (UI-1) — зеркало Rust `commands::agent` ─────────────────────────────────────────────────

/** Уровень автономии прогона (вход `agent_run`, зеркало Rust `normalize_autonomy`): `confirm` — Confirm-тир
 *  ждёт аппрува человека; `auto` — Auto-тир применяется под blast-radius-кэпом без аппрува. */
export type AgentAutonomy = 'confirm' | 'auto';

/** W-4: элемент истории переписки агента (мультитёрн), зеркалит Rust `HistoryMsg`. */
export interface AgentHistoryMsg {
  role: 'user' | 'assistant';
  text: string;
}

/** Статус файла changeset'а (зеркало Rust `AgentFileStatus`): `new` — новая заметка; `edit` — правка. */
export type AgentFileStatus = 'new' | 'edit';

/** Род предложенного действия (зеркало Rust `AgentProposedKind`, serde snake_case): `file` — правка/
 *  создание заметки (путь + ±строки + раскрываемый дифф); `exec` — команда/процесс (рисуется как `$ cmd`
 *  exec-стилем, без ±строк/диффа). ACP-EXEC: exec-permission внешнего ACP-агента (напр. Hermes). */
export type AgentProposedKind = 'file' | 'exec';

/** Один предложенный файл (поверхность аппрува; зеркало Rust `AgentProposedFile`). `actionId` —
 *  адрес решения Approve/Reject (id строки `agent_actions`, передаётся в `agent_approve`). */
export interface AgentProposedFile {
  /** vault-rel путь цели (для `kind:'exec'` — командная строка). */
  path: string;
  /** Добавлено строк (line-diff current → proposed). Для exec — 0. */
  add: number;
  /** Удалено строк. Для exec — 0. */
  del: number;
  status: AgentFileStatus;
  /** file | exec — род действия (ACP-EXEC). Отсутствие на старом проводе → бэкенд дефолтит в `file`. */
  kind: AgentProposedKind;
  /** id строки ledger (state=proposed) — адрес решения в `agent_approve`. */
  actionId: number;
}

/** Статус шага плана (зеркало Rust `AgentPlanStepState`) — для дока «План/Граф» (SUB-2/RES). */
export type AgentPlanStepState = 'pending' | 'running' | 'done' | 'failed';

/** Статус субагента в дереве делегирования (зеркало Rust `AgentSubagentState`). */
export type AgentSubagentState = 'spawned' | 'running' | 'done' | 'failed' | 'paused';

/** Один шаг плана прогона (зеркало Rust `AgentPlanStep`) — узел дока плана. */
export interface AgentPlanStep {
  /** Стабильный id шага (адрес обновления статуса `planStepStatus`). */
  id: string;
  /** Человекочитаемая подпись шага. */
  label: string;
  status: AgentPlanStepState;
}

/**
 * Событие агент-стрима (зеркалит Rust `agent::connect::wire::AgentStreamEvent`, тег `type`, camelCase) —
 * СТАБИЛЬНЫЙ контракт. Реалтайм-поток: `assistantToken` (дельты модели), `toolCall`/`toolResult`
 * (вызов инструмента ДО/ПОСЛЕ, корреляция по `id`), `contextUsage` (загрузка окна → %-бар),
 * `proposal` (changeset, ждёт решения — каждый файл уже `proposed` в ledger) + `diff` (по файлу),
 * `final` (итог хода), `error` (терминальная ошибка хода).
 *
 * W-23 — паритет с бэкендом: `planProposed`/`planStepStatus` (план/граф SUB-2), `subagentStatus`
 * (дерево делегирования SUB-2), `execProposal`/`execResult` (exec-песочница SANDBOX-6c — силуэт+exit-код,
 * БЕЗ сырого stdout: приватность §5.6), `report` (отчёт deep-research RES-5). Рендерятся в W-24/25/26;
 * здесь — приём в контракт + аккумуляция в сторе (иначе события молча терялись).
 */
export type AgentStreamEvent =
  | { type: 'assistantToken'; text: string }
  | { type: 'toolCall'; id: string; kind: string; args: string; title?: string | null }
  | { type: 'toolResult'; id: string; content: string; isError: boolean }
  | { type: 'contextUsage'; used: number; window: number }
  | { type: 'proposal'; runId: number; files: AgentProposedFile[] }
  | { type: 'diff'; path: string; add: number; del: number; status: AgentFileStatus }
  | { type: 'final'; text: string }
  | { type: 'error'; message: string }
  | { type: 'execProposal'; runId: number; actionId: number; summary: string }
  | { type: 'execResult'; runId: number; actionId: number; exitCode: number; finalized: boolean }
  | { type: 'planProposed'; runId: number; steps: AgentPlanStep[] }
  | { type: 'planStepStatus'; id: string; status: AgentPlanStepState }
  | {
      type: 'subagentStatus';
      parentRunId: number;
      childRunId: number;
      goal: string;
      status: AgentSubagentState;
      summary?: string;
    }
  | {
      type: 'report';
      runId: number;
      title: string;
      path: string;
      sourcesCount: number;
      rounds: number;
    };

/** Решение по одному предложенному действию (вход `agent_approve`, зеркало Rust `ApprovalDecision`). */
export interface AgentApprovalDecision {
  /** id строки ledger (из `AgentStreamEvent.proposal.files[].actionId`). */
  actionId: number;
  /** Одобрить (apply) или отклонить (диск не трогаем). */
  approve: boolean;
}

// ── W-38: история переписок агента (левый сайдбар) ──────────────────────────────────────────────────

/** Сводка одной агент-сессии для списка истории (зеркало Rust `AgentSessionDto`). `title` — задача
 *  первого хода; `status` — статус последнего; `turnCount`/`updatedAt` — агрегаты. */
export interface AgentSessionInfo {
  sessionId: string;
  title: string;
  status: string;
  turnCount: number;
  updatedAt: number;
}

/** Один персистированный шаг хода (зеркало Rust `PersistedStepDto`). */
export interface PersistedStep {
  kind: string;
  args: string;
  title: string | null;
  result: string | null;
  isError: boolean;
}

/** Один персистированный ход переписки (зеркало Rust `PersistedTurnDto`). */
export interface PersistedTurn {
  runId: number;
  task: string;
  assistantText: string;
  report: string | null;
  error: string | null;
  status: string;
  createdAt: number;
  steps: PersistedStep[];
}

/** Данные переоткрываемой переписки (зеркало Rust `AgentSessionDataDto`) — ходы в хронологии ASC. */
export interface AgentSessionData {
  turns: PersistedTurn[];
}

// ── W-10: навыки агента (SL-панель) ─────────────────────────────────────────────────────────────────

/** W-10 строка навыка для SL-панели (зеркалит Rust `commands::agent::SkillRowDto`). */
export interface SkillRow {
  name: string;
  description: string;
  /** `'vendor'` (hash-pinned bundle) | `'local'` (TrustedLocal). */
  tier: 'vendor' | 'local';
  relPath: string;
  isVendor: boolean;
  useCount: number;
  lastUsedAt: number | null;
  createdBy: string | null;
  isAgentCreated: boolean;
  pinned: boolean;
  /** `'active'|'stale'|'archived'` (advisory lifecycle) | null. */
  state: 'active' | 'stale' | 'archived' | null;
  license: string | null;
}
/** W-10 снимок SL-панели (зеркалит Rust `commands::agent::SkillListDto`). */
export interface SkillList {
  learningEnabled: boolean;
  skillsDir: string | null;
  skills: SkillRow[];
  parseErrors: number;
}

// ── CONN-4/ACP: подключение агента ──────────────────────────────────────────────────────────────────

/** CONN-4: подключение агента (`ai.connection`) для UI-селектора. `mode` нормализован; `socket` — путь
 *  AF_UNIX для local (`null` → дефолт `<vault>/.nexus/agentd.sock`). `url`/`auth_ref` (CONN-3) не сюда. */
export interface AgentConnectionDto {
  mode: 'embedded' | 'local' | 'remote' | 'acp';
  socket: string | null;
  /** ACP-1b `ai.connection.acpCommand`: командная строка ACP-агента (split по пробелам). `null` → не задан. */
  acpCommand: string | null;
  /** ACP-1b `ai.connection.acpCwd`: рабочая папка спавна ACP-агента. `null` → корень vault. */
  acpCwd: string | null;
  /** ACP-REMOTE-SSH `ai.connection.acpTransport`: `"local"` (спавн команды) | `"ssh"` (сборка ssh-команды).
   *  `null`/пусто → как `"local"`. */
  acpTransport: string | null;
  /** ACP-REMOTE-SSH `ai.connection.acpSshHost` (ssh): `"user@host"`. */
  acpSshHost: string | null;
  /** ACP-REMOTE-SSH `ai.connection.acpSshKey` (ssh): путь к приватному ключу; `null`/пусто → ключ по умолчанию. */
  acpSshKey: string | null;
  /** ACP-REMOTE-SSH `ai.connection.acpRemoteCommand` (ssh): команда запуска ACP-агента НА ХОСТЕ (split по пробелам). */
  acpRemoteCommand: string | null;
}
