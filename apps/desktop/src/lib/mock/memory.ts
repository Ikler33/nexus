/**
 * Мок памяти агента (MEM) для браузерного dev/vitest: in-memory список фактов с той же семантикой,
 * что бэкенд-команды (`memory_list/add/set_pinned/edit/delete`). Без LLM — `propose` всегда `[]`
 * (фронт упадёт на эвристический фолбэк). Пины — сверху, затем по дате создания (как в SQL ORDER BY).
 */
import type { MemoryAddResult, MemoryFact } from '../tauri-api';

let facts: MemoryFact[] = [];
let seq = 1;
let clock = 1_700_000_000; // монотонные «секунды» (без Date — детерминизм в тестах)

const sorted = (): MemoryFact[] =>
  [...facts].sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.createdAt - a.createdAt);

export async function list(): Promise<MemoryFact[]> {
  return sorted();
}

/** Зеркалит бэкенд-контракт: дубль → существующий id с `inserted:false`; новый → `inserted:true`;
 *  пустой текст → `null`. (MEM-5: фронт по `inserted` решает, безопасно ли «Отменить».) */
export async function add(
  text: string,
  source?: 'explicit' | 'auto',
): Promise<MemoryAddResult | null> {
  const t = text.trim();
  if (!t) return null;
  const existing = facts.find((f) => f.text === t);
  if (existing) return { id: existing.id, inserted: false }; // INSERT OR IGNORE — дубль
  const id = seq++;
  facts.push({ id, text: t, pinned: false, source: source ?? 'explicit', createdAt: clock++, usedAt: 0 });
  return { id, inserted: true };
}

export async function setPinned(id: number, pinned: boolean): Promise<void> {
  facts = facts.map((f) => (f.id === id ? { ...f, pinned } : f));
}

export async function edit(id: number, text: string): Promise<void> {
  const t = text.trim();
  if (!t) return;
  facts = facts.map((f) => (f.id === id ? { ...f, text: t } : f));
}

export async function remove(id: number): Promise<void> {
  facts = facts.filter((f) => f.id !== id);
}

export async function propose(): Promise<string[]> {
  return []; // нет мок-LLM → фронт берёт эвристический фолбэк (срез команды); MEM-9: массив
}

/** Сброс для тестов. */
export function __reset(): void {
  facts = [];
  seq = 1;
  clock = 1_700_000_000;
}
