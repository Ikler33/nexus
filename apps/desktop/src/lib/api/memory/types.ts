/**
 * DTO-типы memory-домена (F-2d): курируемые ЯВНЫЕ факты агента (MEM) и предложения консолидации
 * (MEM-8). Зеркала Rust-структур (`memory` / `consolidate`) — контракт провода `invoke`. Потребители
 * импортируют по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

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
