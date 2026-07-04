import * as mockMemory from '../../mock/memory';
import { bridge } from '../bridge';
import type {
  ConsolidationChoice,
  ConsolidationOutcome,
  ConsolidationPlan,
  MemoryAddResult,
  MemoryFact,
} from './types';

/**
 * Memory-домен (F-2d): память агента (MEM) — курируемые ЯВНЫЕ факты о пользователе/проектах: CRUD
 * панели «Память ИИ», авто-предложение (MEM-9) для чипа подтверждения, консолидация (MEM-8, round-trip
 * plan → apply → undo). Все вызовы — через `bridge` (Tauri ↔ мок `lib/mock/memory`); потребители ходят
 * сюда по-прежнему через `tauriApi.memory` (barrel-реэкспорт в `lib/tauri-api.ts`). Вне Tauri —
 * in-memory мок (фича OFF по умолчанию).
 */
export const memory = {
  /** AC-MEM-2: все факты — пины сверху, затем по дате. Вне Tauri — in-memory мок. */
  list: (): Promise<MemoryFact[]> =>
    bridge<MemoryFact[]>('memory_list', undefined, () => mockMemory.list()),

  /** AC-MEM-1/6: добавить факт. `source`: `'explicit'` (по умолч.) или `'auto'` (подтверждённое).
   *  Возвращает `{id, inserted}` (`inserted=false` — дубль, вернули существующий id) или `null`
   *  (пустой текст). MEM-5: `inserted` решает, безопасно ли «Отменить» удалять факт. */
  add: (text: string, source?: 'explicit' | 'auto'): Promise<MemoryAddResult | null> =>
    bridge<MemoryAddResult | null>('memory_add', { text, source }, () =>
      mockMemory.add(text, source),
    ),

  /** AC-MEM-3: пин/анпин факта. */
  setPinned: (id: number, pinned: boolean): Promise<void> =>
    bridge<void>('memory_set_pinned', { id, pinned }, () => mockMemory.setPinned(id, pinned)),

  /** AC-MEM-3: правка текста факта (бэкенд ре-эмбеддит). */
  edit: (id: number, text: string): Promise<void> =>
    bridge<void>('memory_edit', { id, text }, () => mockMemory.edit(id, text)),

  /** AC-MEM-3: удалить факт (+ из индекса). */
  delete: (id: number): Promise<void> =>
    bridge<void>('memory_delete', { id }, () => mockMemory.remove(id)),

  /** AC-MEM-6 (MEM-9): предложить 0..N факт-кандидатов по обмену (быстрая модель). Пустой массив —
   *  нечего предлагать / нет модели. */
  propose: (userText: string, assistantText: string): Promise<string[]> =>
    bridge<string[]>('memory_propose', { userText, assistantText }, () => mockMemory.propose()),

  /** MEM-8 (флаг `aiMemoryConsolidation`): посчитать предложение консолидации факта (read-only,
   *  НИЧЕГО не пишет). Нет основной модели/эмбеддера/индекса → fail-closed `{op:{kind:'add'}}`. */
  consolidatePlan: (text: string, source?: 'explicit' | 'auto'): Promise<ConsolidationPlan> =>
    bridge<ConsolidationPlan>('memory_consolidate_plan', { text, source }, () =>
      mockMemory.consolidatePlan(text, source),
    ),

  /** MEM-8: применить выбор пользователя к предложению (одна транзакция + индексация); возвращает,
   *  что РЕАЛЬНО произошло. */
  consolidateApply: (
    plan: ConsolidationPlan,
    choice: ConsolidationChoice,
  ): Promise<ConsolidationOutcome> =>
    bridge<ConsolidationOutcome>('memory_consolidate_apply', { plan, choice }, () =>
      mockMemory.consolidateApply(plan, choice),
    ),

  /** MEM-8c-b: откатить группу консолидации по `opGroup` (undo авто-режима, §4.6). `true` — что-то
   *  реально откатилось. Optimistic-безопасно (правка юзера не теряется). */
  consolidateUndo: (opGroup: number): Promise<boolean> =>
    bridge<boolean>('memory_consolidate_undo', { opGroup }, () =>
      mockMemory.consolidateUndo(opGroup),
    ),
};
