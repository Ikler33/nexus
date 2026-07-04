/**
 * DTO-типы board-домена (F-2d): канбан-доска (карточки/колонки/scope/конфиг, BOARD-2/3), застрявшие
 * задачи (AI-2a) и плоский список markdown-задач дашборда (TASK-1). Зеркала Rust-структур (`board::*`
 * / `commands::tasks`) — контракт провода `invoke`. Потребители импортируют по-прежнему из
 * `lib/tauri-api` (barrel-реэкспорт).
 */

/** Задача из заметки (TASK-1, дашборд) — зеркало Rust `commands::tasks::TaskItem`. */
export interface TaskItem {
  path: string;
  /** 1-based номер строки задачи. */
  line: number;
  checked: boolean;
  text: string;
  title: string | null;
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
