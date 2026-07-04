import * as mockBoard from '../../mock/board';
import { bridge } from '../bridge';
import type { BoardConfig, BoardData, BoardSummary, StaleTask, TaskCard, TaskItem } from './types';

/**
 * Board-домен (F-2d): задачи vault — канбан-доска (BOARD-2/3: карточки + персист-конфиг колонок/
 * порядка/scope), «застрявшие» задачи (AI-2a) и плоский список markdown-задач дашборда (TASK-1). Всё
 * офлайн, без LLM. Все вызовы — через `bridge` (Tauri ↔ мок `lib/mock/board`); потребители ходят сюда
 * по-прежнему через `tauriApi.board`/`tauriApi.tasks` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const board = {
  list: (statusKey?: string): Promise<TaskCard[]> =>
    bridge<TaskCard[]>('list_board', { statusKey }, () => mockBoard.listBoard()),
  /** Доска целиком: конфиг + карточки в scope; order самозалечивается (GC удалённых). */
  get: (slug?: string): Promise<BoardData> =>
    bridge<BoardData>('get_board', { slug }, () => mockBoard.getBoard()),
  /** Персист конфига доски (переименование колонок, ручной порядок DnD). */
  save: (config: BoardConfig): Promise<void> =>
    bridge<void>('save_board', { config }, () => mockBoard.saveBoard(config)),
  /** Список досок (всегда ≥1 — синтетический дефолт). */
  boards: (): Promise<BoardSummary[]> =>
    bridge<BoardSummary[]>('list_boards', undefined, () => mockBoard.listBoards()),
  /** AI-2a: «застрявшие» задачи — не правленные ≥ thresholdDays (умолч. 14) дней по edit_events. */
  stale: (statusKey?: string, thresholdDays?: number): Promise<StaleTask[]> =>
    bridge<StaleTask[]>('stale_tasks', { statusKey, thresholdDays }, () => mockBoard.staleTasks()),
};

/** Плоский список markdown-задач vault (TASK-1, дашборд) — скан на лету. Вне Tauri — пусто. */
export const tasks = {
  /** Все markdown-задачи vault (TASK-1, дашборд) — скан на лету. Вне Tauri — пусто. */
  listTasks: (): Promise<TaskItem[]> =>
    bridge<TaskItem[]>('list_tasks', undefined, () => mockBoard.listTasks()),
};
