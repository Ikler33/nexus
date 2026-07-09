import { app, external } from './api/app';
import { agent, agentConnection } from './api/agent';
import { backup } from './api/backup';
import { board, tasks } from './api/board';
import { isTauri } from './api/bridge';
import { chat } from './api/chat';
import { contradictions } from './api/contradictions';
import { digest } from './api/digest';
import { egress } from './api/egress';
import { goals, home, homeEvents } from './api/home';
import { graph } from './api/graph';
import { inline } from './api/inline';
import { memory } from './api/memory';
import { episode } from './api/episode';
import { news, newsEvents } from './api/news';
import { plugins } from './api/plugins';
import { properties } from './api/properties';
import { scheduler, schedulerEvents } from './api/scheduler';
import { search } from './api/search';
import { settings as settingsApi, websearch } from './api/settings';
import { git } from './api/git';
import { suggest } from './api/suggest';
import { attachments, vault, vaultEvents } from './api/vault';

/**
 * Barrel фронтового API: весь фронт ходит к нативному слою только через `tauriApi`.
 *
 * Прямой Tauri IPC (`invoke` / `Channel` / `listen`) живёт ТОЛЬКО в слое `lib/api/*` (bridge +
 * доменные модули, контракт §4.1 ARCHITECTURE). Этот файл — ТОНКИЙ barrel: только реэкспорт
 * доменных объектов/типов и сборка `tauriApi` из них (плюс `isTauri`); доменной логики здесь нет.
 *
 * Стадия F-2 REFACTOR-PLAN §4 завершена: домены вынесены в `lib/api/<домен>/{types,index}.ts`
 * (F-2a vault; F-2b chat; F-2c agent+news; F-2d — остаток: app/backup/board/tasks/contradictions/
 * digest/egress/graph/home/goals/inline/memory/episode/plugins/properties/scheduler/search/settings/
 * websearch/suggest/git). 150+ потребителей продолжают импортировать имена/типы из `lib/tauri-api`
 * без правок — barrel их реэкспортирует.
 *
 * Вне Tauri (браузерное превью, vitest) вызовы прозрачно проксируются в мок-бэкенд (`./mock/*`)
 * через `bridge`/`subscribe` — это позволяет вести фронт/дизайн на тех же контрактах параллельно
 * бэкенду (DESIGN §0). Честные исключения из `bridge` (Channel-стримы `chat.streamRag`/`agent.run`/
 * `inline.complete`, OS-диалоги `backup`/`vault.pickDirectory`/`news.exportLogs`, OS-навигация
 * `external.open`) остаются прямыми `invoke`/`window.open` в СВОИХ доменных модулях с комментом.
 */

export { isTauri };

// ── Реэкспорт DTO-типов доменов (контракт баррела — потребители импортируют их из `lib/tauri-api`) ─
export type { BuildInfo } from './api/app/types';
export type { BackupImportReport } from './api/backup/types';
export type { FileEntry, NoteRef, TagCount, VaultInfo } from './api/vault/types';
export type {
  BoardColumn,
  BoardConfig,
  BoardData,
  BoardScope,
  BoardSummary,
  StaleTask,
  TaskCard,
  TaskItem,
} from './api/board/types';
export type {
  BacklinkEntry,
  FullGraph,
  GraphData,
  GraphEdge,
  GraphNode,
  MentionEntry,
} from './api/graph/types';
export type { SearchHit } from './api/search/types';
export type { LinkSuggestion, TagSuggestion } from './api/suggest/types';
export type {
  ContinueNote,
  GoalEntry,
  HeatDay,
  HomeActivity,
  HomeData,
  HomeStats,
  OpenQuestion,
  RecentNote,
  StaleNote,
  Widget,
} from './api/home/types';
export type { NoteProperty, PropertyType } from './api/properties/types';
export type { Digest } from './api/digest/types';
export type { ActiveJob, DeadJob, JobCounts } from './api/scheduler/types';
export type { Contradiction } from './api/contradictions/types';
export type { InlineMode, InlineStreamEvent } from './api/inline/types';
export type { PermissionChip, PluginAuditRecord, PluginInfo } from './api/plugins/types';
export type {
  GitChangeKind,
  GitCommitOutcome,
  GitConflictFile,
  GitFileSecret,
  GitMergePreview,
  GitPullOutcome,
  GitResolution,
  GitSecretKind,
  GitStatusEntry,
} from './api/git/types';
export type {
  ConsolidationChoice,
  ConsolidationOutcome,
  ConsolidationPlan,
  ConsolidationPlanOp,
  MemoryAddResult,
  MemoryFact,
} from './api/memory/types';
export type { EpisodeHit, EpisodeRow } from './api/episode/types';
export type { EgressFeatureId, EgressState } from './api/egress/types';
export type {
  AgentFlagsDto,
  AiConfigDto,
  AiEndpoint,
  SetAiResult,
  WebSearchConfig,
} from './api/settings/types';
// F-2b: DTO-типы chat-домена живут в `lib/api/chat/types.ts`.
export type {
  ChatSearchHit,
  ChatSessionInfo,
  ChatStreamEvent,
  EgressDeniedKind,
  MemoryHit,
  StoredChatMessage,
  WebSource,
} from './api/chat/types';
// F-2c: DTO-типы agent-домена живут в `lib/api/agent/types.ts`.
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
// F-2c: DTO-типы news-домена живут в `lib/api/news/types.ts`.
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

/**
 * Единый фасад нативного API. Каждое поле — доменный объект из `lib/api/<домен>/` (реэкспорт).
 * Составные пространства `events` (подписки нескольких доменов) и `settings` (settings-домен +
 * agent-домен `agentConnection` под прежними именами) собираются здесь — форма/имена сохранены
 * байт-в-байт (потребители не тронуты).
 */
export const tauriApi = {
  app,
  backup,
  external,

  // F-2a: vault-домен (`lib/api/vault/`).
  vault,
  // F-2d: tasks — плоский список markdown-задач (TASK-1), живёт в board-домене.
  tasks,
  // F-2a: вложения — файловые операции vault-домена.
  attachments,

  graph,
  search,
  suggest,
  goals,
  board,
  properties,
  home,
  digest,
  scheduler,
  contradictions,

  /** Событийные подписки: собраны из доменных `*Events` (news/vault/scheduler/home). */
  events: {
    // F-2c: news-домен (`news:progress`).
    onNewsProgress: newsEvents.onNewsProgress,
    // F-2a: watcher-подписки vault-домена (`vault:changed`/`vault:file-changed`/`vault:index-progress`).
    onVaultChanged: vaultEvents.onVaultChanged,
    onFileChanged: vaultEvents.onFileChanged,
    onIndexProgress: vaultEvents.onIndexProgress,
    // F-2d: scheduler-домен (`jobs:changed`) и home-домен (`home:widget-updated`).
    onJobsChanged: schedulerEvents.onJobsChanged,
    onWidgetUpdated: homeEvents.onWidgetUpdated,
  },

  // F-2b: chat-домен (RAG-стрим + сессии переписки).
  chat,
  inline,
  plugins,
  git,
  // F-2c: news-домен (лента/ридер/диагностика).
  news,
  memory,
  // F-2c: agent-домен (прогоны + история + навыки). `agent.run` — честное bridge-исключение (Channel).
  agent,
  episode,
  egress,
  websearch,

  /** Настройки: settings-домен (AI-конфиг/флаги) + agent-домен `agentConnection` (CONN-4/ACP)
   *  под прежними именами `setAgentConnection`/`testAgentConnection`. */
  settings: {
    getAiConfig: settingsApi.getAiConfig,
    setAiConfig: settingsApi.setAiConfig,
    testConnection: settingsApi.testConnection,
    setAgentFlags: settingsApi.setAgentFlags,
    // F-2c: подключение агента живёт в agent-домене (`agentConnection`).
    setAgentConnection: agentConnection.set,
    testAgentConnection: agentConnection.test,
  },
};

export type TauriApi = typeof tauriApi;
