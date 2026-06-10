import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import * as mockEgress from './mock/egress';
import * as mockGit from './mock/git';
import * as mockHome from './mock/home';
import * as mockNews from './mock/news';
import * as mockPlugins from './mock/plugins';
import * as mockSettings from './mock/settings';
import * as mockTags from './mock/tags';
import * as mockVault from './mock/vault';

/**
 * Единственное место в кодовой базе, где разрешён прямой вызов Tauri IPC
 * (`invoke` / `Channel`) — контракт §4.1 ARCHITECTURE. Весь фронт ходит к нативному
 * слою только через `tauriApi`.
 *
 * Вне Tauri (браузерное превью, vitest) методы прозрачно проксируются в мок-бэкенд
 * (`./mock/*`) — это позволяет вести фронт/дизайн на тех же контрактах параллельно
 * бэкенду (DESIGN §0).
 */

/** Запись файлового дерева (зеркалит Rust `vault::FileEntry`). */
export interface FileEntry {
  name: string;
  /** Путь относительно корня vault, разделитель `/`. */
  path: string;
  isDir: boolean;
  hasChildren: boolean;
  sizeBytes: number;
}

/** Сведения об открытом vault (зеркалит Rust `vault::VaultInfo`). */
export interface VaultInfo {
  root: string;
  name: string;
}

/** Лёгкая ссылка на заметку (зеркалит Rust `vault::NoteRef`) — для автокомплита/поиска. */
export interface NoteRef {
  path: string;
  title: string | null;
}

/** Тег с количеством заметок (зеркалит Rust `tags::TagCount`, DP-2 — панель «Теги»). */
export interface TagCount {
  name: string;
  count: number;
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

/** Сводка очереди планировщика для StatusBar (зеркалит Rust `scheduler::JobCounts`, ADR-007 срез 5). */
export interface JobCounts {
  pending: number;
  running: number;
  dead: number;
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

/**
 * Событие RAG-чат-стрима (зеркалит Rust `commands::chat::ChatStreamEvent`, тег `type`, camelCase).
 * Порядок: `sources` → (для reasoning-модели — живые `reasoningSummary`/`reasoning`) → много `token`
 * → `done` (или `error`). `reasoning` — сырой chain-of-thought (спойлер), `reasoningSummary` —
 * короткая живая сводка CoT («💭 …», R1); оба могут не приходить (non-reasoning модель).
 */
/** Типизированный отказ политики эгресса в стриме (AC-EGR-14): offline | feature | host. */
export type EgressDeniedKind = 'offline' | 'feature' | 'host';

export type ChatStreamEvent =
  | { type: 'sources'; sources: SearchHit[] }
  | { type: 'token'; text: string }
  | { type: 'reasoning'; text: string }
  | { type: 'reasoningSummary'; text: string }
  | { type: 'done'; full: string }
  | { type: 'error'; message: string; deniedKind?: EgressDeniedKind };

/** Событие inline-стрима редактора (зеркалит Rust `commands::inline::InlineStreamEvent`). Без `sources`
 * — inline не делает RAG-ретрив (D2). Порядок: много `token` → `done` (или `error`). */
export type InlineStreamEvent =
  | { type: 'token'; text: string }
  | { type: 'done'; full: string }
  | { type: 'error'; message: string };

/** Режим inline-генерации (зеркалит Rust `ai::InlineMode`). */
export type InlineMode = 'continue' | 'rewrite' | 'summarize';

/** AI-эндпоинт настроек (зеркалит Rust `settings::EndpointDto`). `model` опционален. */
export interface AiEndpoint {
  url: string;
  model: string | null;
}
/** Текущая AI-конфигурация для формы настроек (зеркалит Rust `settings::AiConfigDto`). */
export interface AiConfigDto {
  chat: AiEndpoint | null;
  embedding: AiEndpoint | null;
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

/** Запись ленты новостей (зеркалит Rust `news::NewsItem`, NF-3). Время — Unix-секунды. */
export interface NewsItem {
  id: number;
  sourceId: string;
  url: string;
  titleRu: string;
  summaryRu: string;
  topic: string;
  /** Источник русскоязычный (резюме без пометки «перевод»). */
  langRu: boolean;
  publishedAt: number;
  read: boolean;
}

/** Итог последнего прогона ленты (зеркалит Rust `news::NewsRun`): шапка-сводка дня. */
export interface NewsRun {
  runAt: number;
  digestRu: string;
  itemsNew: number;
  sourcesOk: number;
  sourcesTotal: number;
  llmFailed: number;
  /** Видимые ошибки источников («источник: причина») — no silent caps (AC-NF-1). */
  errors: string[];
}

/** Страница ленты (зеркалит Rust `commands::news::NewsPageDto`). */
export interface NewsPage {
  items: NewsItem[];
  topics: string[];
  run: NewsRun | null;
}

/** Конфиг ленты `news.json` (зеркалит Rust `news::NewsConfig`); `enabled` = consent (AC-NF-7). */
export interface NewsConfig {
  enabled: boolean;
  /** Переопределения вкл/выкл источников реестра: id → bool. */
  sources: Record<string, boolean>;
  /** Ключевые слова фильтра; `null` — пресет по умолчанию. */
  keywords: string[] | null;
}

/** Источник реестра v1 (зеркалит Rust `commands::news::NewsSourceDto`) — для consent-строки. */
export interface NewsSource {
  id: string;
  title: string;
  enabled: boolean;
  langRu: boolean;
}

/** Статья reader'а (зеркалит Rust `commands::news::NewsArticleDto`, NF-6). `denied` — хост вне
 * политики эгресса (HN-домены/офлайн): fail-closed, UI отдаёт резюме + ссылку на оригинал. */
export type NewsArticle =
  | { status: 'ready'; paras: string[]; translated: boolean; truncated: boolean }
  | { status: 'denied'; message: string };

/** Запущены ли мы внутри Tauri-webview (а не в обычном браузере / тесте). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export const tauriApi = {
  app: {
    /** Версия нативного приложения (Rust-команда `app_version`). */
    version: () => (isTauri() ? invoke<string>('app_version') : Promise.resolve('dev')),
  },

  vault: {
    /** Открывает vault по абсолютному пути; в браузере — мок. */
    openVault: (path: string) =>
      isTauri() ? invoke<VaultInfo>('open_vault', { path }) : mockVault.openVault(path),

    /** Ленивый листинг каталога (`dirPath` относительный; '' = корень). */
    listDir: (dirPath: string) =>
      isTauri() ? invoke<FileEntry[]>('list_dir', { dirPath }) : mockVault.listDir(dirPath),

    /** Читает содержимое файла vault. */
    readFile: (path: string) =>
      isTauri() ? invoke<string>('read_file', { path }) : mockVault.readFile(path),

    /** Пишет содержимое файла vault. */
    writeFile: (path: string, content: string) =>
      isTauri()
        ? invoke<void>('write_file', { path, content })
        : mockVault.writeFile(path, content),

    /** Заметки vault (path + title) для автокомплита `[[wikilink]]`. #22: опциональный
     * подстрочный `query`-фильтр + `limit` — топ-N вместо всего vault (префиксы ранжируются выше). */
    listNotes: (query?: string, limit?: number) =>
      isTauri()
        ? invoke<NoteRef[]>('list_notes', { query, limit })
        : mockVault.listNotes(query, limit),

    /** Резолвит цель `[[wikilink]]` в путь файла — бэкенд-семантика индексатора (путь / +`.md` /
     * basename, затем алиас V4.1); #22: клик по ссылке без полного списка заметок на фронте. */
    resolveNote: (target: string) =>
      isTauri()
        ? invoke<string | null>('resolve_note', { target })
        : mockVault.resolveNote(target),

    /** Теги vault с количеством заметок — панель «Теги» сайдбара (DP-2). */
    listTags: (): Promise<TagCount[]> =>
      isTauri() ? invoke<TagCount[]>('list_tags') : mockTags.listTags(),

    /** Ручной реиндекс vault (quick action «Переиндексировать», макет home.jsx): фоновый
     * полный обход; по завершении бэкенд шлёт `vault:changed`. В браузере — no-op. */
    rescan: (): Promise<void> => (isTauri() ? invoke<void>('rescan_vault') : Promise.resolve()),

    /** Число живых заметок индекса — статусбар «Проиндексировано · N» (DP-14). Мок — 847,
     * как в демо-данных Home (`lib/mock/home.ts`). */
    notesCount: (): Promise<number> =>
      isTauri() ? invoke<number>('notes_count') : Promise.resolve(847),

    /** Unix-mtime файла (сек) — clock-чип doc-meta превью (DP-15). Мок — «3 ч назад». */
    fileMtime: (path: string): Promise<number> =>
      isTauri()
        ? invoke<number>('file_mtime', { path })
        : Promise.resolve(Math.floor(Date.now() / 1000) - 3 * 3600),

    /** Системный выбор папки vault (нативный диалог Tauri). Вне Tauri — `null`. */
    pickDirectory: async (): Promise<string | null> => {
      if (!isTauri()) return null;
      const picked = await openDialog({ directory: true, multiple: false });
      return typeof picked === 'string' ? picked : null;
    },
  },

  graph: {
    /** Беклинки файла (источник истины — SQLite, ADR-004). */
    getBacklinks: (path: string) =>
      isTauri()
        ? invoke<BacklinkEntry[]>('get_backlinks', { path })
        : mockVault.getBacklinks(path),

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
  },

  goals: {
    /** Все заметки-цели (инлайн-тег `#goal`) с прогрессом (#35). Офлайн, без LLM. Вне Tauri — мок. */
    list: (): Promise<GoalEntry[]> =>
      isTauri() ? invoke<GoalEntry[]>('list_goals') : mockVault.getGoals(),
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
        : Promise.resolve({ pending: 0, running: 0, dead: 0 }),

    /** Идёт ли ещё работа над `kind` (pending|running) — для сброса «Генерирую…», когда джоба
     *  завершилась/упала без нового результата. Вне Tauri — `false`. */
    jobActive: (kind: string): Promise<boolean> =>
      isTauri() ? invoke<boolean>('job_active', { kind }) : Promise.resolve(false),
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
  },

  events: {
    /**
     * Подписка на событие «индекс vault обновлён» (backend `emit("vault:changed")` после реиндекса —
     * ADR-007 S8 event-канал). Возвращает функцию отписки. Вне Tauri — no-op (мок-бэкенд не индексирует).
     */
    onVaultChanged: async (cb: () => void): Promise<() => void> => {
      if (!isTauri()) return () => {};
      return listen('vault:changed', () => cb());
    },

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

  chat: {
    /**
     * RAG-чат со стримингом (Ф1-7): события приходят в `onEvent` (`sources` → `token`… → `done`).
     * Возвращает функцию отмены текущего стрима. Вне Tauri — мок.
     */
    streamRag: (
      question: string,
      onEvent: (event: ChatStreamEvent) => void,
      opts?: { k?: number; center?: string; grounded?: boolean },
    ): (() => void) => {
      if (!isTauri())
        return mockVault.streamChat(question, onEvent, { k: opts?.k, grounded: opts?.grounded });
      const channel = new Channel<ChatStreamEvent>();
      channel.onmessage = onEvent;
      invoke<void>('chat_rag', {
        question,
        k: opts?.k,
        center: opts?.center,
        grounded: opts?.grounded,
        channel,
      }).catch((e: unknown) => onEvent({ type: 'error', message: String(e) }));
      return () => {
        void invoke<void>('chat_cancel');
      };
    },
  },

  inline: {
    /**
     * Inline-генерация в редакторе (IL-1/2): стрим результата в `onEvent` (`token`… → `done`|`error`).
     * `mode` — `continue`/`rewrite`/`summarize`; `context` — текст до курсора; `selection` — выделение
     * (для rewrite/summarize). Возвращает функцию отмены (взводит `inline_cancel`). Вне Tauri — мок.
     */
    complete: (
      mode: InlineMode,
      context: string,
      selection: string | undefined,
      onEvent: (event: InlineStreamEvent) => void,
    ): (() => void) => {
      if (!isTauri()) return mockVault.streamInline(mode, onEvent);
      const channel = new Channel<InlineStreamEvent>();
      channel.onmessage = onEvent;
      invoke<void>('inline_complete', { mode, context, selection, channel }).catch((e: unknown) =>
        onEvent({ type: 'error', message: String(e) }),
      );
      return () => {
        void invoke<void>('inline_cancel');
      };
    },
  },

  plugins: {
    /** Установленные плагины vault (`.nexus/plugins/*`) со статусом совместимости (Ф0-13/Ф2). */
    list: (): Promise<PluginInfo[]> =>
      isTauri() ? invoke<PluginInfo[]>('list_plugins') : mockPlugins.list(),

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
      isTauri() ? invoke<GitCommitOutcome>('git_commit', { message }) : mockGit.commit(),

    /** Выборочный коммит (#10): коммитит ТОЛЬКО выбранные пути (из `git.status()`), а не всё-или-ничего.
     *  Secret-scan по выбранным; устаревший/пустой выбор → `nothing-to-commit`. Вне Tauri — мок. */
    commitPaths: (paths: string[], message?: string): Promise<GitCommitOutcome> =>
      isTauri()
        ? invoke<GitCommitOutcome>('git_commit_paths', { paths, message })
        : mockGit.commit(),

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

  /** Лента AI-новостей (NF-3/NF-5, спека `docs/specs/news-feed.md`). Прогон гоняет планировщик
   * (kind `newsfeed`); готовность — событие `jobs:changed`. Вне Tauri — стейтфул-мок. */
  news: {
    /** Страница ленты: записи (свежие сверху) + чипы тем + последний прогон. */
    page: (opts?: { topic?: string; unreadOnly?: boolean; page?: number }): Promise<NewsPage> =>
      isTauri()
        ? invoke<NewsPage>('get_news', {
            topic: opts?.topic,
            unreadOnly: opts?.unreadOnly,
            page: opts?.page,
          })
        : mockNews.page(opts),

    /** Отметка прочитано/непрочитано (AC-NF-9). */
    markRead: (id: number, read: boolean): Promise<void> =>
      isTauri() ? invoke<void>('news_mark_read', { id, read }) : mockNews.markRead(id, read),

    /** «В заметку» (AC-NF-11): создаёт `News/<дата> <заголовок>.md`, возвращает путь заметки. */
    toNote: (id: number): Promise<string> =>
      isTauri() ? invoke<string>('news_to_note', { id }) : mockNews.toNote(id),

    /** Ручной прогон «Обновить» (AC-NF-6): ставит джобу с дедупом; `false` — уже в очереди. */
    refresh: (): Promise<boolean> =>
      isTauri() ? invoke<boolean>('refresh_news') : mockNews.refresh(),

    /** Конфиг `news.json` (consent + источники + ключи). */
    getConfig: (): Promise<NewsConfig> =>
      isTauri() ? invoke<NewsConfig>('get_news_config') : mockNews.getConfig(),

    /** Сохраняет конфиг и мгновенно синхронизирует политику эгресса (NF-4, AC-NF-7). */
    setConfig: (config: NewsConfig): Promise<NewsConfig> =>
      isTauri() ? invoke<NewsConfig>('set_news_config', { config }) : mockNews.setConfig(config),

    /** Реестр источников v1 с действующими флагами — consent показывает, куда пойдут запросы. */
    sources: (): Promise<NewsSource[]> =>
      isTauri() ? invoke<NewsSource[]>('news_sources') : mockNews.sources(),

    /** Полный текст статьи для reader (NF-6): кэш → guarded-фетч → RU-перевод. Долгий вызов. */
    article: (id: number): Promise<NewsArticle> =>
      isTauri() ? invoke<NewsArticle>('news_article', { id }) : mockNews.article(id),

    /** «Сократить» (NF-6): 3–6 RU-тезисов по тексту статьи. */
    summarize: (id: number): Promise<string[]> =>
      isTauri() ? invoke<string[]>('news_summarize', { id }) : mockNews.summarize(id),
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

  settings: {
    /** Текущая AI-конфигурация из `.nexus/local.json` — для префилла формы (раздел «AI / Модели»). */
    getAiConfig: (): Promise<AiConfigDto> =>
      isTauri() ? invoke<AiConfigDto>('get_ai_config') : mockSettings.getAiConfig(),

    /**
     * Записывает AI-конфиг в `.nexus/local.json` (сохраняя прочие ключи) и ГОРЯЧО применяет chat.
     * `embeddingChanged` в ответе → UI просит перезапуск (индексатор перечитает конфиг при старте).
     */
    setAiConfig: (chat: AiEndpoint | null, embedding: AiEndpoint | null): Promise<SetAiResult> =>
      isTauri()
        ? invoke<SetAiResult>('set_ai_config', { chat, embedding })
        : mockSettings.setAiConfig(chat, embedding),

    /** Проверка связи с LLM-эндпоинтом (пробный GET `/v1/models`). Резолвится = достижим; throw = нет. */
    testConnection: (url: string): Promise<void> =>
      isTauri() ? invoke<void>('test_ai_connection', { url }) : mockSettings.testConnection(url),
  },
};

export type TauriApi = typeof tauriApi;
