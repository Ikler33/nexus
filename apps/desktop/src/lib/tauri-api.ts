import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import * as mockBoard from './mock/board';
import * as mockProps from './mock/properties';
import * as mockEgress from './mock/egress';
import * as mockMemory from './mock/memory';
import * as mockEpisode from './mock/episode';
import * as mockGit from './mock/git';
import * as mockHome from './mock/home';
import * as mockBackup from './mock/backup';
import * as mockPlugins from './mock/plugins';
import * as mockSettings from './mock/settings';
import * as mockTags from './mock/tags';
import * as mockVault from './mock/vault';
import { agent, agentConnection } from './api/agent';
import type { AgentConnectionDto } from './api/agent/types';
import { isTauri } from './api/bridge';
import { chat } from './api/chat';
import { news, newsEvents } from './api/news';
import { attachments, vault, vaultEvents } from './api/vault';
import type { NoteRef } from './api/vault/types';

/**
 * Barrel фронтового API: весь фронт ходит к нативному слою только через `tauriApi`.
 *
 * Прямой Tauri IPC (`invoke` / `Channel`) разрешён только в слое `lib/api/*` (bridge +
 * доменные модули) и — до конца распила F-2 — в этом файле (контракт §4.1 ARCHITECTURE).
 * Вынесенные домены (F-2a: vault; F-2b: chat; F-2c: agent + news) реэкспортируются отсюда —
 * 140+ потребителей продолжают импортировать из `lib/tauri-api` без правок; остальные домены
 * мигрируют в `lib/api/<домен>/` следующими срезами (F-2d+).
 *
 * Вне Tauri (браузерное превью, vitest) методы прозрачно проксируются в мок-бэкенд
 * (`./mock/*`) — это позволяет вести фронт/дизайн на тех же контрактах параллельно
 * бэкенду (DESIGN §0).
 */

// F-2a: DTO-типы vault-домена живут в `lib/api/vault/types.ts` (реэкспорт — контракт баррела).
export { isTauri };
export type { FileEntry, NoteRef, TagCount, VaultInfo } from './api/vault/types';
// F-2b: DTO-типы chat-домена живут в `lib/api/chat/types.ts` (реэкспорт — контракт баррела).
export type {
  ChatSearchHit,
  ChatSessionInfo,
  ChatStreamEvent,
  EgressDeniedKind,
  MemoryHit,
  StoredChatMessage,
  WebSource,
} from './api/chat/types';
// F-2c: DTO-типы agent-домена живут в `lib/api/agent/types.ts` (реэкспорт — контракт баррела).
export type {
  AgentApprovalDecision,
  AgentAutonomy,
  AgentConnectionDto,
  AgentFileStatus,
  AgentHistoryMsg,
  AgentPlanStep,
  AgentPlanStepState,
  AgentProposedFile,
  AgentProposedKind,
  AgentSessionData,
  AgentSessionInfo,
  AgentStreamEvent,
  AgentSubagentState,
  PersistedStep,
  PersistedTurn,
  SkillList,
  SkillRow,
} from './api/agent/types';
// F-2c: DTO-типы news-домена живут в `lib/api/news/types.ts` (реэкспорт — контракт баррела).
export type {
  LlmDownInfo,
  NewsArticle,
  NewsConfig,
  NewsEndpointHealth,
  NewsItem,
  NewsPage,
  NewsRun,
  NewsSource,
} from './api/news/types';

/** Git-версия сборки (W-20, зеркалит Rust `BuildInfo`). */
export interface BuildInfo {
  version: string;
  branch: string;
  hash: string;
  dirty: boolean;
}

/** Задача из заметки (TASK-1, дашборд) — зеркало Rust `commands::tasks::TaskItem`. */
export interface TaskItem {
  path: string;
  /** 1-based номер строки задачи. */
  line: number;
  checked: boolean;
  text: string;
  title: string | null;
}

/** Предложение авто-тега (AI-2c, зеркалит Rust `tagger::TagSuggestion`). `tags` УЖЕ отфильтрованы по
 *  словарю vault (closed-vocab); `dropped` — сколько модель выдала вне словаря (телеметрия). */
export interface TagSuggestion {
  tags: string[];
  dropped: number;
}

/** Чип права плагина (зеркалит Rust `plugin::PermissionChip`, DP-8): уровень риска для UI. */
export interface PermissionChip {
  kind: string;
  detail: string;
  level: 'safe' | 'caution' | 'sensitive';
}

/** Статус установленного плагина (зеркалит Rust `plugin::PluginInfo`). */
export interface PluginInfo {
  dir: string;
  id: string | null;
  name: string | null;
  version: string | null;
  compatible: boolean;
  error: string | null;
  /** Сводка прав манифеста — чипы и consent-sheet (DP-8). */
  permissions: PermissionChip[];
  /** Включён ли плагин (персист `plugins.<dir>.enabled`, дефолт ВКЛ). Выключенный не открывает сессию. */
  enabled: boolean;
}

/** git-sync: статус файла (зеркалит Rust `git::StatusEntry`/`ChangeKind`). */
export type GitChangeKind = 'new' | 'modified' | 'deleted' | 'renamed' | 'other';
export interface GitStatusEntry {
  path: string;
  kind: GitChangeKind;
}
/** Тип найденного секрета (зеркалит Rust `git::SecretKind`). */
export type GitSecretKind =
  | 'private-key'
  | 'openai-key'
  | 'github-token'
  | 'aws-access-key'
  | 'slack-token';
export interface GitFileSecret {
  path: string;
  findings: { line: number; kind: GitSecretKind }[];
}
/** Исход авто-коммита (зеркалит Rust `git::CommitOutcome`, тег `status`). */
export type GitCommitOutcome =
  | { status: 'nothing-to-commit' }
  | { status: 'blocked-by-secrets'; findings: GitFileSecret[] }
  | { status: 'committed'; oid: string; message: string; files: number };

/** Исход pull/sync (зеркалит Rust `git::PullOutcome`, тег `status`). */
export type GitPullOutcome =
  | { status: 'up-to-date' }
  | { status: 'fast-forward'; oid: string }
  | { status: 'merge-required' };

/** Конфликтный файл 3-way (зеркалит Rust `git::ConflictFile`). `null` = файла нет в этой версии. */
export interface GitConflictFile {
  path: string;
  base: string | null;
  ours: string | null;
  theirs: string | null;
}

/** Превью merge (зеркалит Rust `git::MergePreview`). */
export type GitMergePreview =
  | { status: 'up-to-date' }
  | { status: 'clean'; theirs: string }
  | { status: 'conflicts'; theirs: string; files: GitConflictFile[] };

/** Резолв одного файла: путь + итоговое содержимое (для `git_resolve_conflicts`). */
export type GitResolution = [path: string, content: string];

/** Результат гибридного поиска по телу (зеркалит Rust `search::SearchHit`). */
export interface SearchHit {
  chunkId: number;
  path: string;
  title: string | null;
  headingPath: string | null;
  snippet: string;
  /** Слитый RRF-score (вектор + FTS); шкала относительная, для сортировки. */
  score: number;
}

/** Предложенная связь (зеркалит Rust `suggest::LinkSuggestion`). */
export interface LinkSuggestion {
  path: string;
  title: string | null;
  /** max-sim score (косинус, относительный — для сортировки/порога). */
  score: number;
  /** «Причина» — сниппет лучшего совпавшего чанка целевой заметки. */
  reason: string;
}

/** Заметка-цель (зеркалит Rust `goals::Goal`). `progress` 0–100 или `null` (нет валидного значения, D7). */
export interface GoalEntry {
  path: string;
  title: string | null;
  progress: number | null;
}

/** Карточка задачи доски (зеркалит Rust `board::TaskCard`, BOARD-2). `status` — raw-значение frontmatter
 *  (колонкование на фронте); project/priority/due опц.; tags из `file_tags` (отсортированы). */
export interface TaskCard {
  path: string;
  title: string | null;
  status: string;
  project: string | null;
  priority: string | null;
  due: string | null;
  tags: string[];
}

/** Застрявшая задача (AI-2a, зеркалит Rust `board::StaleTask`): не правленная дольше порога. `lastEdit` —
 *  unix-сек последнего наблюдённого изменения (edit_events, фолбэк mtime); `daysStale` = дней простоя. */
export interface StaleTask {
  path: string;
  title: string | null;
  status: string;
  lastEdit: number;
  daysStale: number;
}

/** Колонка доски (зеркалит Rust `board::config::BoardColumn`, BOARD-3). `id` = raw-значение `status`;
 *  `label` пусто → локализация на фронте; `doneLike` — терминальная колонка. */
export interface BoardColumn {
  id: string;
  label: string;
  wip: number | null;
  color: string | null;
  doneLike: boolean;
}
/** Scope доски (folder-префикс / project / superset тегов). */
export interface BoardScope {
  folder: string | null;
  project: string | null;
  tags: string[];
}
/** Конфиг доски (персист `.nexus/boards/<id>.json`, BOARD-3). */
export interface BoardConfig {
  id: string;
  title: string;
  statusKey: string;
  columns: BoardColumn[];
  scope: BoardScope;
  order: Record<string, string[]>;
  sort: string;
  cardFields: string[];
}
/** Доска целиком: конфиг + карточки в его scope; `corrupt` — JSON битый (фронт-тост, дефолт). */
export interface BoardData {
  config: BoardConfig;
  cards: TaskCard[];
  corrupt: boolean;
}
/** Сводка доски для списка/переключателя. */
export interface BoardSummary {
  id: string;
  title: string;
}

/** Тип свойства (виджет Properties-панели, PROP-2; зеркалит Rust `properties::PropertyType`). */
export type PropertyType = 'text' | 'list' | 'number' | 'checkbox' | 'date' | 'datetime' | 'tags';
/** Свойство заметки: плоский frontmatter-скаляр + разрешённый тип (реестр+эвристика). */
export interface NoteProperty {
  key: string;
  value: string;
  type: PropertyType;
}

/** HOME-дашборд: статические/динамические виджеты (зеркалит Rust `home::HomeData`, H1/DP-1). LLM-виджеты —
 *  отдельным API (H2+, см. `docs/dev/HOME_BACKEND_PLAN.md`). */
export interface HomeStats {
  notes: number;
  tags: number;
  links: number;
  words: number;
}
/** Недавняя заметка с метой (DP-1: карточке «Недавние» нужны время и объём). */
export interface RecentNote {
  path: string;
  title: string | null;
  updatedAt: number;
  words: number;
}
export interface HomeData {
  stats: HomeStats;
  recent: RecentNote[];
  goals: GoalEntry[];
}

/** День heatmap активности (зеркалит Rust `home::activity::HeatDay`): 0 = сегодня. */
export interface HeatDay {
  daysAgo: number;
  count: number;
}
/** «Продолжить»: последняя правленая заметка со сниппетом (DP-1). */
export interface ContinueNote {
  path: string;
  title: string | null;
  updatedAt: number;
  words: number;
  snippet: string;
}
/** Зона «Активность» HOME (зеркалит Rust `home::activity::ActivityData`, H6).
 *  Всё выведено из ТЕКУЩИХ mtime файлов (истории правок нет — честные приближения). */
export interface HomeActivity {
  heatmap: HeatDay[];
  changesToday: number;
  week: number;
  prevWeek: number;
  streakDays: number;
  bestStreak: number;
  orphans: number;
  continue: ContinueNote | null;
}

/** Кэшированный LLM-виджет HOME (зеркалит Rust `home::widgets::Widget`, H2). `content` непрозрачен
 *  (текст/JSON — парсит конкретный виджет). `stale` — vault менялся с момента генерации (кэш устарел);
 *  `status` — `ready` (контент валиден) | `error` (последний refresh упал, показан прежний контент). */
export interface Widget {
  key: string;
  content: string;
  generatedAt: number;
  sourceHash: number;
  status: string;
  stale: boolean;
}

/** Устаревшая заметка «Stale radar» (зеркалит Rust `home::stale::StaleNote`, H4). Слой 1 — `score`/
 *  `severity` (`red`|`orange`)/`ageDays` + флаги-сигналы; слой 2 (`reason`/`action`/`hint`) — из кэша
 *  LLM (`null`, пока не обогащено). `action` — `update`|`archive`|`split`|`delete`. */
export interface StaleNote {
  path: string;
  title: string | null;
  score: number;
  severity: string;
  ageDays: number;
  isDraft: boolean;
  isWip: boolean;
  isOverdue: boolean;
  isOrphan: boolean;
  isEvergreen: boolean;
  reason: string | null;
  action: string | null;
  hint: string | null;
}

/** Открытый вопрос «Open questions» (H5, зона 4): текст вопроса + путь заметки-источника. Контент
 *  виджета `open_questions` — JSON-массив таких объектов (зеркалит Rust `home::insights`). */
export interface OpenQuestion {
  question: string;
  path: string;
}

/** Дайджест недавних изменений (зеркалит Rust `digest::Digest`, ADR-007 slice 4). Время — Unix-секунды. */
export interface Digest {
  createdAt: number;
  since: number;
  content: string;
  noteCount: number;
}

/** Отчёт импорта бэкапа (#59, зеркалит Rust `backup::ImportReport`). */
export interface BackupImportReport {
  factsAdded: number;
  factsSkipped: number;
  sessionsAdded: number;
  sessionsReused: number;
  messagesAdded: number;
  messagesSkipped: number;
  episodesAdded: number;
  episodesSkipped: number;
  skillsAdded: number;
  skillsSkipped: number;
  messagesOrphaned: number;
  episodesOrphaned: number;
  schemaVersionMismatch: boolean;
}

/** Сводка очереди планировщика для StatusBar (зеркалит Rust `scheduler::JobCounts`, ADR-007 срез 5). */
export interface JobCounts {
  /** Всего ожидающих (в т.ч. запланированные на будущее recurring) — для тултипа/модалки. */
  pending: number;
  /** Готовы к запуску сейчас (`pending` с наступившим `run_at`) — только это «работа сейчас». */
  ready: number;
  running: number;
  dead: number;
}

/** Активная фоновая джоба (зеркалит Rust `scheduler::ActiveJob`) — модалка очереди за «N задач». */
export interface ActiveJob {
  id: number;
  kind: string;
  state: 'running' | 'pending';
  /** Когда джоба готова к запуску (unix-секунды); для running — момент последнего перехода. */
  runAt: number;
  attempts: number;
}

/** Мёртвая фоновая джоба (зеркалит Rust `scheduler::DeadJob`) — детали для модалки за «⚠ N». */
export interface DeadJob {
  id: number;
  kind: string;
  attempts: number;
  lastError: string | null;
  /** Когда перешла в dead (unix-секунды). */
  updatedAt: number;
}

/** Найденное противоречие (зеркалит Rust `contradictions::Contradiction`). `ctype` — hard|soft|temporal. */
export interface Contradiction {
  pathA: string;
  pathB: string;
  ctype: string;
  explanation: string;
  createdAt: number;
}

/** Обратная ссылка (зеркалит Rust `graph::BacklinkEntry`). */
export interface BacklinkEntry {
  sourcePath: string;
  sourceTitle: string | null;
  context: string | null;
  lineNumber: number | null;
}

/** Незалинкованное упоминание (зеркалит Rust `graph::MentionEntry`). */
export interface MentionEntry {
  sourcePath: string;
  sourceTitle: string | null;
  snippet: string;
}

/** Узел/ребро/данные локального графа (зеркалит Rust `graph::*`). */
export interface GraphNode {
  id: number;
  path: string;
  title: string | null;
  /** Теги заметки (без `#`, отсортированы) — цвет узла и фильтр-чипы графа. */
  tags: string[];
}
export interface GraphEdge {
  source: number;
  target: number;
}
export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
}
/** Единый граф всего vault (зеркалит Rust `graph::FullGraph`). */
export interface FullGraph {
  nodes: GraphNode[];
  edges: GraphEdge[];
  /** Всего не-удалённых файлов в vault. */
  totalFiles: number;
  /** Показаны не все узлы (обрезано по степени связности). */
  truncated: boolean;
}

/** Конфиг web-агента (W-3, зеркалит Rust `WebSearchConfig`): URL SearXNG = consent на эгресс к нему. */
export interface WebSearchConfig {
  enabled: boolean;
  url: string;
}

/** Эпизод памяти (EP-2, зеркалит Rust `episode::EpisodeHit`) — саммари прошлой сессии. По клику грузит
 *  сессию (`sessionId`). `summarySnippet` — обрезанное саммари; `started/endedAt` — unix-секунды. */
export interface EpisodeHit {
  episodeId: number;
  sessionId: number;
  sessionTitle: string;
  summarySnippet: string;
  startedAt: number;
  endedAt: number;
  score: number;
}

/** Эпизод для панели (EP-3, зеркалит Rust `episode::EpisodeRow`) — полная строка + темы + флаг
 *  скрытия. `topics` — распарсенный JSON; `dismissed` — скрыт из ретривала (обратимо). */
export interface EpisodeRow {
  id: number;
  sessionId: number;
  sessionTitle: string;
  summary: string;
  topics: string[];
  startedAt: number;
  endedAt: number;
  generatedAt: number;
  dismissed: boolean;
}

/** Факт памяти агента (MEM, зеркалит Rust `memory::MemoryFact`). `source`: 'explicit' | 'auto' (D1).
 *  `createdAt`/`usedAt` — unix-секунды; `usedAt=0` — ещё не подмешивался в контекст. */
export interface MemoryFact {
  id: number;
  text: string;
  pinned: boolean;
  source: string;
  createdAt: number;
  usedAt: number;
}

/** Результат `memory.add` (MEM-5): id факта + `inserted` (новая строка vs дубль). */
export interface MemoryAddResult {
  id: number;
  inserted: boolean;
}

/** MEM-8: предложенная операция консолидации (зеркалит Rust `consolidate::PlanOp`, serde tag `kind`).
 *  `add` — добавить новый; `update` — дополнить `targetId` («было `oldText` → станет `newText`»);
 *  `supersede` — новый противоречит `targetId` (старый устарел); `noop` — уже покрыт `coveredBy`. */
export type ConsolidationPlanOp =
  | { kind: 'add' }
  | {
      kind: 'update';
      targetId: number;
      oldText: string;
      newText: string;
      /** Источник целевого факта ('explicit'|'auto') — авто-режим не переписывает молча explicit (§4.3). */
      targetSource: string;
    }
  | { kind: 'supersede'; targetId: number; oldText: string; targetSource: string }
  | { kind: 'noop'; coveredBy: number };

/** MEM-8: предложение консолидации (round-trip plan → чип → apply). */
export interface ConsolidationPlan {
  candidate: string;
  source: string;
  op: ConsolidationPlanOp;
}

/** MEM-8: выбор пользователя на чипе предложения — `accept` (применить op) / `keepSeparate`
 *  (оставить как есть, просто добавить кандидата новым фактом). */
export type ConsolidationChoice = 'accept' | 'keepSeparate';

/** MEM-8: что РЕАЛЬНО произошло в БД (зеркалит Rust `ConsolidationOutcome`, serde tag `op`). */
export type ConsolidationOutcome =
  | { op: 'add'; id: number; inserted: boolean }
  | { op: 'update'; id: number; oldText: string; newText: string; opGroup: number }
  | {
      op: 'supersede';
      id: number;
      supersededId: number;
      oldText: string;
      newText: string;
      inserted: boolean;
      opGroup: number;
    }
  | { op: 'noop' };

// F-2c: wire-типы агента (AgentStreamEvent и спутники, W-38 история, W-10 навыки, CONN/ACP
// подключение) живут в `lib/api/agent/types.ts` (реэкспорт выше — контракт баррела).

/** Событие inline-стрима редактора (зеркалит Rust `commands::inline::InlineStreamEvent`). Без `sources`
 * — inline не делает RAG-ретрив (D2). Порядок: много `token` → `done` (или `error`). */
export type InlineStreamEvent =
  | { type: 'token'; text: string }
  | { type: 'done'; full: string }
  | { type: 'error'; message: string };

/** Режим inline-генерации (зеркалит Rust `ai::InlineMode`). `prompt` — свободный запрос (⌘/ prompt-box). */
export type InlineMode = 'continue' | 'rewrite' | 'summarize' | 'prompt';

/** AI-эндпоинт настроек (зеркалит Rust `settings::EndpointDto`). `model` опционален. */
export interface AiEndpoint {
  url: string;
  model: string | null;
}
/** Текущая AI-конфигурация для формы настроек (зеркалит Rust `settings::AiConfigDto`).
 *  `AgentConnectionDto` (CONN-4 `ai.connection`) — F-2c: живёт в `lib/api/agent/types.ts`. */
export interface AiConfigDto {
  chat: AiEndpoint | null;
  embedding: AiEndpoint | null;
  /** Утилитарная мелкая модель (`ai.fast`) — inline/судья/новости. */
  fast: AiEndpoint | null;
  /** CONN-4 `ai.connection`: режим подключения агента (embedded|local|remote) + сокет для local. */
  connection: AgentConnectionDto;
  // Agent-флаги в `.nexus/local.json`. ПОСЛЕ AGENT-0.2/0.6 десктоп-`agent_run` ЧИТАЕТ часть рантаймом
  // (`agentActuatorEnabled`/`ai.web`/`ai.agent_skills_dir`) — тогглы управляют И десктоп-агентом Castor,
  // И headless `nexus-agentd`. Автономию прогона десктоп берёт per-run из UI. См. AgentFlagsDto.
  /** `ai.agent_autonomy` («confirm»|«auto»): дефолт-постура headless-коннектора. `null` → confirm. */
  agentAutonomy: string | null;
  /** `ai.agent_actuator_enabled`: мастер-свитч РЕАЛЬНЫХ действий агента в vault (default-OFF → заглушки). */
  agentActuatorEnabled: boolean;
  /** `ai.sandbox_enabled`: мастер-свитч OS-песочницы (Linux-only). Предпосылка shell-exec. */
  sandboxEnabled: boolean;
  /** `ai.shell_enable`: host-exec в песочнице (Confirm, никогда Auto). Требует sandbox + Linux. */
  shellEnable: boolean;
  /** `ai.web.allow_public_fetch`: снимает allowlist с агентского `web.fetch` (публичный egress). */
  webAllowPublicFetch: boolean;
  /** W-10 `ai.skills.learning_enabled`: owner-gated самообучение (агент авторствует навыки). */
  skillsLearningEnabled: boolean;
  /** W-10 `ai.agent_skills_dir`: каталог SKILL.md (отн. vault или абсолютный). `null` — навыков нет. */
  agentSkillsDir: string | null;
  /** W-24 `ai.delegation.enabled`: owner-gated делегирование субагентам (default-OFF). */
  delegationEnabled: boolean;
  /** W-25 `ai.research.enabled`: owner-gated deep-research (default-OFF). Требует delegation+web+actuator. */
  researchEnabled: boolean;
  /** Поддержана ли песочница/host-exec на ЭТОЙ платформе (Linux-only) — фронт дизейблит sandbox/shell. */
  shellSupported: boolean;
}

/** Записываемый поднабор agent-флагов (зеркалит Rust `settings::AgentFlagsDto`). */
export interface AgentFlagsDto {
  /** «confirm»|«auto»; иное/`null` → дефолт confirm (ключ не пишется в local.json). */
  agentAutonomy: string | null;
  /** `ai.agent_actuator_enabled`: мастер-свитч реальных vault-действий агента (default-OFF). */
  agentActuatorEnabled: boolean;
  sandboxEnabled: boolean;
  shellEnable: boolean;
  webAllowPublicFetch: boolean;
  /** W-10 `ai.skills.learning_enabled` (owner-gated, default-OFF). */
  skillsLearningEnabled: boolean;
  /** W-10 `ai.agent_skills_dir`: каталог навыков (пусто/`null` → ключ убирается). */
  agentSkillsDir: string | null;
  /** W-24 `ai.delegation.enabled` (owner-gated, default-OFF). */
  delegationEnabled: boolean;
  /** W-25 `ai.research.enabled` (owner-gated, default-OFF). */
  researchEnabled: boolean;
}

/** Снимок политики эгресса ядра (зеркалит Rust `net::EgressState`; срез 2 net.md). */
export interface EgressState {
  /** Kill-switch «офлайн» (E2): публичные хосты отрезаны, LAN/loopback живут. */
  offline: boolean;
  chat: boolean;
  embed: boolean;
  probe: boolean;
}
/** Сетевая фича ядра (E6); Web/NewsFeed/CloudFallback придут со срезами 3–4. */
export type EgressFeatureId = 'chat' | 'embed' | 'probe';
/** Результат записи AI-конфига (зеркалит Rust `settings::SetAiResult`). */
export interface SetAiResult {
  /** Chat применён немедленно (без перезапуска). */
  chatApplied: boolean;
  /** Embedding изменился → нужен перезапуск приложения для переиндексации. */
  embeddingChanged: boolean;
}

// F-2c: DTO-типы news-домена (NewsItem/NewsRun/NewsConfig/… W-39/40) живут в
// `lib/api/news/types.ts` (реэкспорт выше — контракт баррела).

/** Мок web-конфига для браузер-превью W-3 (in-memory). */
let mockWebSearch: WebSearchConfig = { enabled: false, url: '' };

export const tauriApi = {
  app: {
    /** Версия нативного приложения (Rust-команда `app_version`). */
    version: () => (isTauri() ? invoke<string>('app_version') : Promise.resolve('dev')),
    /**
     * Git-версия сборки (W-20): `{ version, branch, hash, dirty }`, захвачена `build.rs` на
     * компиляции. Статусбар рисует `ветка @ хеш`, чтобы видеть, ЧТО запущено. Вне Tauri
     * (браузер-превью) — отметка `dev`.
     */
    buildInfo: (): Promise<BuildInfo> =>
      isTauri()
        ? invoke<BuildInfo>('app_build_info')
        : Promise.resolve({ version: 'dev', branch: 'dev', hash: '', dirty: false }),
  },

  /** #59 (W-9) backup/restore: экспорт/импорт «второго мозга» (факты/переписка/эпизоды/навыки) в
   *  файл. fs делается в доверенном бэкенде; путь выбирает пользователь OS-диалогом (dialog:default). */
  backup: {
    /** Экспорт в файл через save-диалог. Путь сохранённого файла, либо null если отменили. */
    exportToFile: async (): Promise<string | null> => {
      if (!isTauri()) return mockBackup.exportToFile();
      const path = await saveDialog({
        defaultPath: 'orvin-backup.json',
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (!path) return null;
      await invoke<void>('backup_export_to_path', { path });
      return path;
    },
    /** Импорт из файла через open-диалог. Отчёт импорта, либо null если отменили. */
    importFromFile: async (): Promise<BackupImportReport | null> => {
      if (!isTauri()) return mockBackup.importFromFile();
      const path = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (!path || typeof path !== 'string') return null;
      return invoke<BackupImportReport>('backup_import_from_path', { path });
    },
  },

  external: {
    /**
     * Открывает http(s)-URL в СИСТЕМНОМ браузере (Rust-команда `open_external` через
     * tauri-plugin-opener). В Tauri-вебвью `<a target="_blank">` не открывает браузер (строгий CSP
     * глотает навигацию) — поэтому все внешние ссылки (NF-6 «Оригинал», web-источники чата, ссылки
     * в превью заметок) идут СЮДА. Иные схемы (file:, javascript:) отклоняются и тут, и в Rust.
     * Вне Tauri (браузерное превью) — `window.open`. Открытие — НЕ эгресс приложения (фетчит ОС).
     */
    open: (url: string): Promise<void> => {
      if (!/^https?:\/\//i.test(url)) return Promise.reject(new Error('схема не разрешена'));
      if (!isTauri()) {
        window.open(url, '_blank', 'noopener,noreferrer');
        return Promise.resolve();
      }
      return invoke<void>('open_external', { url });
    },
  },

  // F-2a: vault-домен вынесен в `lib/api/vault/` (вызовы через bridge) — здесь только реэкспорт.
  vault,

  tasks: {
    /** Все markdown-задачи vault (TASK-1, дашборд) — скан на лету. Вне Tauri — пусто. */
    listTasks: () => (isTauri() ? invoke<TaskItem[]>('list_tasks') : Promise.resolve([] as TaskItem[])),
  },

  // F-2a: вложения — файловые операции vault-домена, живут в `lib/api/vault/`.
  attachments,

  graph: {
    /** Беклинки файла (источник истины — SQLite, ADR-004). */
    getBacklinks: (path: string) =>
      isTauri()
        ? invoke<BacklinkEntry[]>('get_backlinks', { path })
        : mockVault.getBacklinks(path),

    /** UNLINK-1: незалинкованные упоминания заголовка файла (FTS-фраза по телу, без уже-линкующих). */
    unlinkedMentions: (path: string) =>
      isTauri()
        ? invoke<MentionEntry[]>('get_unlinked_mentions', { path })
        : mockVault.getUnlinkedMentions(path),

    /** Локальный N-hop граф вокруг файла (ADR-004). */
    getLocalGraph: (center: string, hops: number) =>
      isTauri()
        ? invoke<GraphData>('get_local_graph', { center, hops })
        : mockVault.getLocalGraph(center, hops),

    /** Единый граф всего vault — топ-`limit` файлов по связности (AC-DOD-Ф3). */
    getFullGraph: (limit: number) =>
      isTauri()
        ? invoke<FullGraph>('get_full_graph', { limit })
        : mockVault.getFullGraph(limit),
  },

  search: {
    /** Поиск по title/path/tags (метаданные, Ф0). */
    searchVault: (query: string) =>
      isTauri() ? invoke<NoteRef[]>('search_vault', { query }) : mockVault.searchVault(query),

    /**
     * Гибридный поиск по ТЕЛУ (вектор + FTS5 (+граф) → RRF, §6.2). `limit` по умолчанию 10.
     * `folder`/`tag` — префильтр по метаданным ДО KNN; `center` — открытый файл (граф-ранг).
     */
    searchContent: (
      query: string,
      opts?: { limit?: number; folder?: string; tag?: string; center?: string },
    ) =>
      isTauri()
        ? invoke<SearchHit[]>('search_content', {
            query,
            limit: opts?.limit,
            folder: opts?.folder,
            tag: opts?.tag,
            center: opts?.center,
          })
        : mockVault.searchContent(query, opts),
  },

  suggest: {
    /** Предложения связей для файла (режим 1 max-sim, Ф1-9). Вне Tauri — мок. */
    forFile: (path: string, limit?: number) =>
      isTauri()
        ? invoke<LinkSuggestion[]>('get_link_suggestions', { path, limit })
        : mockVault.getLinkSuggestions(path, limit),

    /** «Похожие заметки» (#35, дискавери — включая уже связанные). Порог — на стороне UI. Вне Tauri — мок. */
    related: (path: string, limit?: number) =>
      isTauri()
        ? invoke<LinkSuggestion[]>('get_related_notes', { path, limit })
        : mockVault.getRelatedNotes(path, limit),

    /** Inspector «Резюме»: краткое LLM-резюме текущего текста заметки (one-shot, не-стрим). `null` =
     *  нет утилитарной модели / пустой текст / пустой ответ → фронт показывает заглушку. Вне Tauri — мок. */
    noteSummary: (text: string): Promise<string | null> =>
      isTauri() ? invoke<string | null>('get_note_summary', { text }) : mockVault.noteSummary(text),

    /** AIP-10: короткое LLM-объяснение связи пары заметок (вместо сырого сниппета; кэш на бэке).
     *  Пустая строка = нет утилитарной модели / ошибка / нет контента → фронт показывает сниппет.
     *  Вне Tauri — '' (естественный фолбэк на сниппет). */
    explainRelation: (pathA: string, pathB: string): Promise<string> =>
      isTauri() ? invoke<string>('explain_relation', { pathA, pathB }) : Promise.resolve(''),

    /** AIP-SQ: до 3 коротких стартовых вопросов по активной заметке `center` для пустого чата.
     *  Пустой список = нет утилитарной модели / нет контента / ошибка LLM → фронт показывает
     *  статические подсказки. Вне Tauri — [] (естественный фолбэк на статику). */
    startingQuestions: (center?: string): Promise<string[]> =>
      isTauri() ? invoke<string[]>('get_starting_questions', { center }) : Promise.resolve([]),

    /** AI-2c: closed-vocab авто-тег — `chat_util` предлагает теги ТОЛЬКО из словаря vault. `tags` уже
     *  отфильтрованы по словарю; пустой список = нет утилитарной модели / нет контента / нет тегов → фронт
     *  показывает «нет предложений». НЕ пишет. Вне Tauri — мок (зеркалит контракт: vocab-фильтр + пусто). */
    suggestTags: (path: string): Promise<TagSuggestion> =>
      isTauri() ? invoke<TagSuggestion>('suggest_tags', { path }) : mockTags.suggestTags(),
  },

  goals: {
    /** Все заметки-цели (инлайн-тег `#goal`) с прогрессом (#35). Офлайн, без LLM. Вне Tauri — мок. */
    list: (): Promise<GoalEntry[]> =>
      isTauri() ? invoke<GoalEntry[]>('list_goals') : mockVault.getGoals(),
  },

  /** Канбан-доска (BOARD-2/3): задачи + персист-конфиг (колонки/порядок/scope). Офлайн, без LLM. */
  board: {
    list: (statusKey?: string): Promise<TaskCard[]> =>
      isTauri() ? invoke<TaskCard[]>('list_board', { statusKey }) : mockBoard.listBoard(),
    /** Доска целиком: конфиг + карточки в scope; order самозалечивается (GC удалённых). */
    get: (slug?: string): Promise<BoardData> =>
      isTauri() ? invoke<BoardData>('get_board', { slug }) : mockBoard.getBoard(),
    /** Персист конфига доски (переименование колонок, ручной порядок DnD). */
    save: (config: BoardConfig): Promise<void> =>
      isTauri() ? invoke<void>('save_board', { config }) : mockBoard.saveBoard(config),
    /** Список досок (всегда ≥1 — синтетический дефолт). */
    boards: (): Promise<BoardSummary[]> =>
      isTauri() ? invoke<BoardSummary[]>('list_boards') : mockBoard.listBoards(),
    /** AI-2a: «застрявшие» задачи — не правленные ≥ thresholdDays (умолч. 14) дней по edit_events. */
    stale: (statusKey?: string, thresholdDays?: number): Promise<StaleTask[]> =>
      isTauri()
        ? invoke<StaleTask[]>('stale_tasks', { statusKey, thresholdDays })
        : mockBoard.staleTasks(),
  },

  /** Реестр типов свойств (PROP-2, Obsidian Properties). Тип глобален по имени; иначе — эвристика. */
  properties: {
    /** Весь реестр явных типов (имя → тип). */
    types: (): Promise<Record<string, PropertyType>> =>
      isTauri() ? invoke<Record<string, PropertyType>>('get_property_types') : mockProps.types(),
    /** Задать явный тип свойства (глобально по имени). */
    setType: (key: string, type: PropertyType): Promise<void> =>
      isTauri() ? invoke<void>('set_property_type', { key, ty: type }) : mockProps.setType(key, type),
    /** Свойства заметки с разрешённым типом (для Properties-панели PROP-3). */
    forNote: (path: string): Promise<NoteProperty[]> =>
      isTauri() ? invoke<NoteProperty[]>('get_note_properties', { path }) : mockProps.forNote(),
  },

  /** HOME-дашборд (бэкенд H1/H2/H6; страница — DP-1). Вне Tauri — стейтфул-мок с контентом макета. */
  home: {
    /** Статические/динамические данные HOME (stats/recent/goals) одним запросом, без LLM. */
    data: (): Promise<HomeData> =>
      isTauri() ? invoke<HomeData>('get_home_data') : mockHome.data(),

    /** Зона «Активность» (H6): heatmap правок, серия дней, сироты, «Продолжить» со сниппетом.
     *  `tzOffsetMin` = `new Date().getTimezoneOffset()` — дни считаются в локали пользователя. */
    activity: (): Promise<HomeActivity> =>
      isTauri()
        ? invoke<HomeActivity>('get_home_activity', {
            tzOffsetMin: new Date().getTimezoneOffset(),
          })
        : mockHome.activity(),

    /** Кэшированный LLM-виджет по ключу (или `null`, если ещё не генерировался). Мгновенно — НЕ ждёт
     *  LLM (генерация фоном; готовность — событие `home:widget-updated`). H2. Известные ключи:
     *  `'daily_brief'` (H3, зона 2), `'open_questions'` (H5, зона 4, manual), `'context_drift'`
     *  (H5, зона 5, scheduled). Для последних двух есть типизированные хелперы ниже. */
    widget: (key: string): Promise<Widget | null> =>
      isTauri() ? invoke<Widget | null>('get_widget', { key }) : mockHome.widget(key),

    /** Ручной refresh виджета (manual): ставит фоновую генерацию в очередь (требует зарегистрированный
     *  виджет; дедуп активной джобы). Завершение — событие `home:widget-updated`. H2. */
    refresh: (key: string): Promise<void> =>
      isTauri() ? invoke<void>('refresh_widget', { key }) : mockHome.refresh(key),

    /** «Stale radar» (H4, зона 4): ранжированный список устаревших заметок. Слой 1 (скоринг) мгновенно
     *  on-open; слой 2 (LLM-причина/действие/подсказка) — из кэша, если обогащали. */
    staleRadar: (): Promise<StaleNote[]> =>
      isTauri() ? invoke<StaleNote[]>('get_stale_radar') : mockHome.staleRadar(),

    /** Ручной запуск LLM-обогащения «Stale radar» (слой 2, manual): топ-N → причина/действие/подсказка,
     *  кэш 24ч. Требует chat; дедуп активной джобы. Завершение — событие `home:widget-updated`
     *  (ключ `'stale_radar'`). Вне Tauri — no-op. */
    staleRefresh: (): Promise<void> =>
      isTauri() ? invoke<void>('refresh_stale_radar') : Promise.resolve(),

    /** Состояние тоггла «Инсайты» (проактивные ИИ-виджеты Home: открытые вопросы + дрейф контекста +
     *  stale-radar). Persisted, дефолт OFF. Вне Tauri — мок. */
    insightsGetEnabled: (): Promise<boolean> =>
      isTauri() ? invoke<boolean>('insights_get_enabled') : mockHome.insightsGetEnabled(),

    /** Переключить «Инсайты»; при включении бэкенд ставит kick-джобы доступных виджетов. Вне Tauri — мок. */
    insightsSetEnabled: (on: boolean): Promise<void> =>
      isTauri() ? invoke<void>('insights_set_enabled', { on }) : mockHome.insightsSetEnabled(on),

    /** «Open questions» (H5, зона 4, manual): незакрытые вопросы из последних заметок — распарсенный
     *  контент виджета `open_questions`. Сгенерировать/обновить — `home.refresh('open_questions')`;
     *  готовность — событие `onWidgetUpdated`. Пока не сгенерировано — `[]`. */
    openQuestions: async (): Promise<OpenQuestion[]> => {
      const w = await tauriApi.home.widget('open_questions');
      if (!w?.content) return [];
      try {
        return JSON.parse(w.content) as OpenQuestion[];
      } catch {
        return [];
      }
    },

    /** «Context drift» (H5, зона 5, scheduled): абзац расхождения «текущий фокус vs цели» — контент
     *  виджета `context_drift` (или `null`, если ещё не сгенерировано/пусто). Обновляется раз в сутки
     *  в фоне; принудительно — `home.refresh('context_drift')`. */
    contextDrift: async (): Promise<string | null> => {
      const w = await tauriApi.home.widget('context_drift');
      return w?.content ? w.content : null;
    },
  },

  digest: {
    /** Последний сгенерированный «Дайджест изменений» (или `null`). ADR-007 slice 4. Вне Tauri — мок. */
    latest: (): Promise<Digest | null> =>
      isTauri() ? invoke<Digest | null>('get_latest_digest') : mockVault.getDigest(),

    /**
     * Ставит генерацию дайджеста в очередь (воркер выполнит на ближайшем тике). Требует
     * сконфигурированного chat (иначе backend вернёт ошибку). Завершение — по событию `jobs:changed`.
     */
    generate: (): Promise<void> =>
      isTauri() ? invoke<void>('generate_digest') : Promise.resolve(),
  },

  scheduler: {
    /** Счётчики джоб (pending/running/dead) для индикатора в StatusBar (ADR-007 срез 5). Вне Tauri — нули. */
    counts: (): Promise<JobCounts> =>
      isTauri()
        ? invoke<JobCounts>('get_job_counts')
        : Promise.resolve({ pending: 0, ready: 0, running: 0, dead: 0 }),

    /** Идёт ли ещё работа над `kind` (pending|running) — для сброса «Генерирую…», когда джоба
     *  завершилась/упала без нового результата. Вне Tauri — `false`. */
    jobActive: (kind: string): Promise<boolean> =>
      isTauri() ? invoke<boolean>('job_active', { kind }) : Promise.resolve(false),

    /** Активные джобы (running/pending) — модалка очереди за «N задач». Вне Tauri — пусто. */
    /** Перезапуск воркера планировщика (N1, аварийная кнопка в модалке очереди). Вне Tauri — no-op. */
    restart: (): Promise<void> =>
      isTauri() ? invoke<void>('restart_scheduler') : Promise.resolve(),

    activeJobs: (): Promise<ActiveJob[]> =>
      isTauri() ? invoke<ActiveJob[]>('get_active_jobs') : Promise.resolve([]),

    /** Детали dead-джоб (kind/ошибка/попытки/когда) — модалка за «⚠ N» в StatusBar. Вне Tauri — пусто. */
    deadJobs: (): Promise<DeadJob[]> =>
      isTauri() ? invoke<DeadJob[]>('get_dead_jobs') : Promise.resolve([]),

    /** «Повторить» dead-джобу: pending с чистыми attempts. `false` — уже не dead (гонка), не ошибка. */
    retryDead: (id: number): Promise<boolean> =>
      isTauri() ? invoke<boolean>('retry_dead_job', { id }) : Promise.resolve(false),

    /** Удалить все dead-джобы («Очистить» в модалке). Возвращает число удалённых. */
    clearDead: (): Promise<number> =>
      isTauri() ? invoke<number>('clear_dead_jobs') : Promise.resolve(0),
  },

  contradictions: {
    /** Найденные противоречия (или `[]`). #vision, спека `docs/specs/contradictions.md`. Вне Tauri — мок. */
    list: (): Promise<Contradiction[]> =>
      isTauri() ? invoke<Contradiction[]>('get_contradictions') : mockVault.getContradictions(),

    /**
     * Ставит поиск противоречий в очередь (воркер выполнит фоном). Требует chat + эмбеддинги; дедуп
     * активной джобы. Завершение — по событию `jobs:changed`. Вне Tauri — no-op.
     */
    generate: (): Promise<void> =>
      isTauri() ? invoke<void>('generate_contradictions') : Promise.resolve(),

    /** Состояние тоггла «Поиск противоречий» (persisted, дефолт OFF). Вне Tauri — мок. */
    getEnabled: (): Promise<boolean> =>
      isTauri() ? invoke<boolean>('contradictions_get_enabled') : mockVault.contradictionsGetEnabled(),

    /** Переключить «Поиск противоречий»; при включении бэкенд ставит kick-джобу. Вне Tauri — мок. */
    setEnabled: (on: boolean): Promise<void> =>
      isTauri()
        ? invoke<void>('contradictions_set_enabled', { on })
        : mockVault.contradictionsSetEnabled(on),
  },

  events: {
    // F-2c: подписка news-домена (`news:progress`) живёт в `lib/api/news/` — здесь реэкспорт.
    onNewsProgress: newsEvents.onNewsProgress,

    // F-2a: watcher-подписки vault-домена (`vault:changed` / `vault:file-changed` /
    // `vault:index-progress`) живут в `lib/api/vault/` — здесь реэкспорт для потребителей.
    onVaultChanged: vaultEvents.onVaultChanged,
    onFileChanged: vaultEvents.onFileChanged,
    onIndexProgress: vaultEvents.onIndexProgress,

    /**
     * Подписка на «очередь задач изменилась» (backend `emit("jobs:changed")` после продуктивного тика
     * воркера — ADR-007). Используется для refetch дайджеста по завершении джобы. Вне Tauri — no-op.
     */
    onJobsChanged: async (cb: () => void): Promise<() => void> => {
      if (!isTauri()) return () => {};
      return listen('jobs:changed', () => cb());
    },

    /**
     * Подписка на «HOME-виджет обновился» (backend `emit("home:widget-updated", key)` после записи кэша
     * виджета — H2). Колбэк получает ключ виджета → фронт перечитывает его `tauriApi.home.widget(key)`.
     * Возвращает функцию отписки. Вне Tauri — no-op (мок-бэкенд не генерирует виджеты).
     */
    onWidgetUpdated: async (cb: (key: string) => void): Promise<() => void> => {
      if (!isTauri()) return () => {};
      return listen<string>('home:widget-updated', (e) => cb(e.payload));
    },
  },

  // F-2b: chat-домен (RAG-стрим + сессии переписки) вынесен в `lib/api/chat/` — здесь реэкспорт.
  chat,

  inline: {
    /**
     * Inline-генерация в редакторе (IL-1/2): стрим результата в `onEvent` (`token`… → `done`|`error`).
     * `mode` — `continue`/`rewrite`/`summarize`/`prompt`; `context` — текст до курсора (или вся заметка
     * как контекст для `prompt`); `selection` — выделение (rewrite/summarize); `prompt` — свободный
     * запрос пользователя (⌘/ prompt-box). Возвращает функцию отмены (взводит `inline_cancel`). Вне
     * Tauri — мок.
     */
    complete: (
      mode: InlineMode,
      context: string,
      selection: string | undefined,
      onEvent: (event: InlineStreamEvent) => void,
      prompt?: string,
    ): (() => void) => {
      if (!isTauri()) return mockVault.streamInline(mode, onEvent, prompt);
      const channel = new Channel<InlineStreamEvent>();
      channel.onmessage = onEvent;
      invoke<void>('inline_complete', { mode, context, selection, prompt, channel }).catch(
        (e: unknown) => onEvent({ type: 'error', message: String(e) }),
      );
      return () => {
        void invoke<void>('inline_cancel');
      };
    },
  },

  plugins: {
    /** Установленные плагины vault (`.nexus/plugins/*`) со статусом совместимости + `enabled` (Ф0-13/Ф2). */
    list: (): Promise<PluginInfo[]> =>
      isTauri() ? invoke<PluginInfo[]>('list_plugins') : mockPlugins.list(),

    /** Включить/выключить плагин (персист). Выключенный не открывает новую сессию. Вне Tauri — мок. */
    setEnabled: (dir: string, on: boolean): Promise<void> =>
      isTauri()
        ? invoke<void>('set_plugin_enabled', { dir, on })
        : mockPlugins.setEnabled(dir, on),

    /** Удалить плагин: каталог → в корзину (.nexus/.trash, обратимо) + очистка настроек. Вне Tauri — мок. */
    remove: (dir: string): Promise<void> =>
      isTauri() ? invoke<void>('remove_plugin', { dir }) : mockPlugins.remove(dir),

    /**
     * Открывает сессию плагина (`.nexus/plugins/<dir>`) → **capability-токен** (§7.9). Токен живёт
     * на host-стороне (в релее), плагину НЕ передаётся (identity по порту/токену, ADR-002).
     */
    openSession: (dir: string): Promise<string> =>
      isTauri() ? invoke<string>('plugin_open_session', { dir }) : mockPlugins.openSession(dir),

    /**
     * Host-функция плагина через брокер: `authorize` (scope + audit) → dispatch. `method` —
     * `vault.readFile`/`vault.listFiles`/`vault.writeFile`. Результат — JSON (контент/записи/`{ok}`).
     */
    invoke: (token: string, method: string, path?: string, content?: string): Promise<unknown> =>
      isTauri()
        ? invoke<unknown>('plugin_invoke', { token, method, path, content })
        : mockPlugins.invoke(token, method, path, content),

    /** Закрывает сессию плагина (отзыв токена в брокере). Зовётся при размонтировании плагина. */
    closeSession: (token: string): Promise<void> =>
      isTauri() ? invoke<void>('plugin_close_session', { token }) : mockPlugins.closeSession(token),
  },

  git: {
    /** Статус рабочего дерева vault (изменённые/новые/удалённые, без игнорируемых). Ф3. */
    status: (): Promise<GitStatusEntry[]> =>
      isTauri() ? invoke<GitStatusEntry[]>('git_status') : mockGit.status(),

    /** Коммит изменений: secret-scan → при находке блокировка; пустое сообщение → авто-саммари. */
    commit: (message?: string): Promise<GitCommitOutcome> =>
      isTauri() ? invoke<GitCommitOutcome>('git_commit', { message }) : mockGit.commit(message),

    /** Выборочный коммит (#10): коммитит ТОЛЬКО выбранные пути (из `git.status()`), а не всё-или-ничего.
     *  Secret-scan по выбранным; устаревший/пустой выбор → `nothing-to-commit`. Вне Tauri — мок. */
    commitPaths: (paths: string[], message?: string): Promise<GitCommitOutcome> =>
      isTauri()
        ? invoke<GitCommitOutcome>('git_commit_paths', { paths, message })
        : mockGit.commitPaths(paths, message),

    /** Сохранить токен доступа к remote в системном keychain (на диск не пишется). Ф3-3b. */
    setToken: (token: string): Promise<void> =>
      isTauri() ? invoke<void>('git_set_token', { token }) : mockGit.setToken(token),

    /** Удалить токен из keychain. */
    clearToken: (): Promise<void> =>
      isTauri() ? invoke<void>('git_clear_token') : mockGit.clearToken(),

    /** Есть ли сохранённый токен (для UI «подключено»). */
    hasToken: (): Promise<boolean> =>
      isTauri() ? invoke<boolean>('git_has_token') : mockGit.hasToken(),

    /** Установить URL remote `origin`. */
    setRemote: (url: string): Promise<void> =>
      isTauri() ? invoke<void>('git_set_remote', { url }) : mockGit.setRemote(url),

    /** URL remote `origin` (или null). */
    getRemote: (): Promise<string | null> =>
      isTauri() ? invoke<string | null>('git_get_remote') : mockGit.getRemote(),

    /** Синхронизация с remote: pull (ff) → push. Токен берётся из keychain. */
    sync: (): Promise<GitPullOutcome> =>
      isTauri() ? invoke<GitPullOutcome>('git_sync') : mockGit.sync(),

    /** Превью merge с origin (in-memory): up-to-date / clean / конфликты (3-way). Ф4-8. */
    mergePreview: (): Promise<GitMergePreview> =>
      isTauri() ? invoke<GitMergePreview>('git_merge_preview') : mockGit.mergePreview(),

    /** Применить разрешённый merge (resolutions: [path, content]) + push. Возвращает oid коммита. */
    resolveConflicts: (theirs: string, resolutions: GitResolution[]): Promise<string> =>
      isTauri()
        ? invoke<string>('git_resolve_conflicts', { theirs, resolutions })
        : mockGit.resolveConflicts(theirs, resolutions),
  },

  // F-2c: news-домен (лента NF-3/NF-5 + ридер NF-6 + диагностика W-39/40) вынесен в
  // `lib/api/news/` — здесь реэкспорт.
  news,

  /** Память агента (MEM): курируемые ЯВНЫЕ ФАКТЫ о пользователе/проектах. MEM-3 — захват:
   *  явное добавление + авто-предложение (`propose`) для чипа подтверждения. CRUD-обёртки для панели
   *  «Память ИИ» добавляются в MEM-4. Вне Tauri — no-op (фича OFF по умолчанию). */
  memory: {
    /** AC-MEM-2: все факты — пины сверху, затем по дате. Вне Tauri — in-memory мок. */
    list: (): Promise<MemoryFact[]> =>
      isTauri() ? invoke<MemoryFact[]>('memory_list') : mockMemory.list(),

    /** AC-MEM-1/6: добавить факт. `source`: `'explicit'` (по умолч.) или `'auto'` (подтверждённое).
     *  Возвращает `{id, inserted}` (`inserted=false` — дубль, вернули существующий id) или `null`
     *  (пустой текст). MEM-5: `inserted` решает, безопасно ли «Отменить» удалять факт. */
    add: (text: string, source?: 'explicit' | 'auto'): Promise<MemoryAddResult | null> =>
      isTauri()
        ? invoke<MemoryAddResult | null>('memory_add', { text, source })
        : mockMemory.add(text, source),

    /** AC-MEM-3: пин/анпин факта. */
    setPinned: (id: number, pinned: boolean): Promise<void> =>
      isTauri() ? invoke<void>('memory_set_pinned', { id, pinned }) : mockMemory.setPinned(id, pinned),

    /** AC-MEM-3: правка текста факта (бэкенд ре-эмбеддит). */
    edit: (id: number, text: string): Promise<void> =>
      isTauri() ? invoke<void>('memory_edit', { id, text }) : mockMemory.edit(id, text),

    /** AC-MEM-3: удалить факт (+ из индекса). */
    delete: (id: number): Promise<void> =>
      isTauri() ? invoke<void>('memory_delete', { id }) : mockMemory.remove(id),

    /** AC-MEM-6 (MEM-9): предложить 0..N факт-кандидатов по обмену (быстрая модель). Пустой массив —
     *  нечего предлагать / нет модели. */
    propose: (userText: string, assistantText: string): Promise<string[]> =>
      isTauri()
        ? invoke<string[]>('memory_propose', { userText, assistantText })
        : mockMemory.propose(),

    /** MEM-8 (флаг `aiMemoryConsolidation`): посчитать предложение консолидации факта (read-only,
     *  НИЧЕГО не пишет). Нет основной модели/эмбеддера/индекса → fail-closed `{op:{kind:'add'}}`. */
    consolidatePlan: (text: string, source?: 'explicit' | 'auto'): Promise<ConsolidationPlan> =>
      isTauri()
        ? invoke<ConsolidationPlan>('memory_consolidate_plan', { text, source })
        : mockMemory.consolidatePlan(text, source),

    /** MEM-8: применить выбор пользователя к предложению (одна транзакция + индексация); возвращает,
     *  что РЕАЛЬНО произошло. */
    consolidateApply: (
      plan: ConsolidationPlan,
      choice: ConsolidationChoice,
    ): Promise<ConsolidationOutcome> =>
      isTauri()
        ? invoke<ConsolidationOutcome>('memory_consolidate_apply', { plan, choice })
        : mockMemory.consolidateApply(plan, choice),

    /** MEM-8c-b: откатить группу консолидации по `opGroup` (undo авто-режима, §4.6). `true` — что-то
     *  реально откатилось. Optimistic-безопасно (правка юзера не теряется). */
    consolidateUndo: (opGroup: number): Promise<boolean> =>
      isTauri()
        ? invoke<boolean>('memory_consolidate_undo', { opGroup })
        : mockMemory.consolidateUndo(opGroup),
  },

  // F-2c: agent-домен (прогоны UI-1 + история W-38 + навыки W-10) вынесен в `lib/api/agent/` —
  // здесь реэкспорт. `agent.run` — честное bridge-исключение (Channel), коммент в домене.
  agent,

  /** Эпизодическая память (EP-3): панель эпизодов + обратимость + тоггл. Вне Tauri — in-memory мок. */
  episode: {
    /** Все эпизоды для панели (обратная хронология, со скрытыми). */
    list: (): Promise<EpisodeRow[]> =>
      isTauri() ? invoke<EpisodeRow[]>('episode_list') : mockEpisode.list(),
    /** Скрыть эпизод (обратимо — убирает из ретривала, строка/вектор живы). */
    dismiss: (id: number): Promise<void> =>
      isTauri() ? invoke<void>('episode_dismiss', { id }) : mockEpisode.dismiss(id),
    /** Восстановить скрытый эпизод. */
    restore: (id: number): Promise<void> =>
      isTauri() ? invoke<void>('episode_restore', { id }) : mockEpisode.restore(id),
    /** Удалить эпизод НАВСЕГДА (строка + вектор). Необратимо; первоисточник-сессия цел. */
    purge: (id: number): Promise<void> =>
      isTauri() ? invoke<void>('episode_purge', { id }) : mockEpisode.purge(id),
    /** Текущее состояние тоггла эпизодической памяти (persisted). */
    getEnabled: (): Promise<boolean> =>
      isTauri() ? invoke<boolean>('episode_get_enabled') : mockEpisode.getEnabled(),
    /** Переключить эпизодическую память; ВКЛ enqueue'ит kick-генерацию (контракт MAJOR-2). */
    setEnabled: (on: boolean): Promise<void> =>
      isTauri() ? invoke<void>('episode_set_enabled', { on }) : mockEpisode.setEnabled(on),
  },

  /** Политика эгресса ядра (срез 2 net.md): тоггл «офлайн» (E2) + per-feature opt-in (E6).
   * Изменения применяются мгновенно и переживают рестарт (E5, OS config-dir). */
  egress: {
    getState: (): Promise<EgressState> =>
      isTauri() ? invoke<EgressState>('get_egress_state') : mockEgress.getState(),

    /** Включение дорезает активный chat-стрим (E10); LAN/loopback-модели продолжают работать. */
    setOffline: (offline: boolean): Promise<EgressState> =>
      isTauri() ? invoke<EgressState>('set_egress_offline', { offline }) : mockEgress.setOffline(offline),

    setFeature: (feature: EgressFeatureId, enabled: boolean): Promise<EgressState> =>
      isTauri()
        ? invoke<EgressState>('set_egress_feature', { feature, enabled })
        : mockEgress.setFeature(feature, enabled),
  },

  /** Web-агент (W-3): consent-конфиг SearXNG (URL = разрешение на эгресс к нему). Вне Tauri — память. */
  websearch: {
    getConfig: (): Promise<WebSearchConfig> =>
      isTauri()
        ? invoke<WebSearchConfig>('get_websearch_config')
        : Promise.resolve(mockWebSearch),
    setConfig: (config: WebSearchConfig): Promise<WebSearchConfig> => {
      if (!isTauri()) {
        mockWebSearch = { ...config };
        return Promise.resolve(mockWebSearch);
      }
      return invoke<WebSearchConfig>('set_websearch_config', { config });
    },
  },

  settings: {
    /** Текущая AI-конфигурация из `.nexus/local.json` — для префилла формы (раздел «AI / Модели»). */
    getAiConfig: (): Promise<AiConfigDto> =>
      isTauri() ? invoke<AiConfigDto>('get_ai_config') : mockSettings.getAiConfig(),

    /**
     * Записывает AI-конфиг в `.nexus/local.json` (сохраняя прочие ключи) и ГОРЯЧО применяет chat.
     * `embeddingChanged` в ответе → UI просит перезапуск (индексатор перечитает конфиг при старте).
     */
    setAiConfig: (
      chat: AiEndpoint | null,
      embedding: AiEndpoint | null,
      fast: AiEndpoint | null = null,
    ): Promise<SetAiResult> =>
      isTauri()
        ? invoke<SetAiResult>('set_ai_config', { chat, embedding, fast })
        : mockSettings.setAiConfig(chat, embedding, fast),

    /** Проверка связи с LLM-эндпоинтом (пробный GET `/v1/models`). Резолвится = достижим; throw = нет. */
    testConnection: (url: string): Promise<void> =>
      isTauri() ? invoke<void>('test_ai_connection', { url }) : mockSettings.testConnection(url),

    /**
     * Персистит agent-флаги (агентд-only) в `.nexus/local.json`. В ОТЛИЧИЕ от setAiConfig — без
     * hot-apply/egress-ресинка: эти флаги читает только headless-агентд при старте. Мгновенно.
     * Возвращает нормализованный набор (невалидная autonomy → `null` = confirm).
     */
    setAgentFlags: (flags: AgentFlagsDto): Promise<AgentFlagsDto> =>
      isTauri()
        ? invoke<AgentFlagsDto>('set_agent_flags', { flags })
        : mockSettings.setAgentFlags(flags),

    // F-2c: подключение агента (CONN-4/ACP — персист режима + проба связи) живёт в
    // `lib/api/agent/` (`agentConnection`) — здесь реэкспорт под прежними именами.
    setAgentConnection: agentConnection.set,
    testAgentConnection: agentConnection.test,
  },
};

export type TauriApi = typeof tauriApi;
