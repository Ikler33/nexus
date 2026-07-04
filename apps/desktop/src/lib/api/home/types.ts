/**
 * DTO-типы home-домена (F-2d): HOME-дашборд (статические/динамические виджеты H1/DP-1, активность H6,
 * LLM-виджеты H2, stale radar H4, open questions H5) и заметки-цели (#35, часть `HomeData`). Зеркала
 * Rust-структур (`home::*` / `goals::Goal`) — контракт провода `invoke`. Потребители импортируют
 * по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

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
