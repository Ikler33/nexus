/**
 * Мок памяти агента (MEM) для браузерного dev/vitest: in-memory список фактов с той же семантикой,
 * что бэкенд-команды (`memory_list/add/set_pinned/edit/delete`). Без LLM — `propose` всегда `[]`
 * (фронт упадёт на эвристический фолбэк). Пины — сверху, затем по дате создания (как в SQL ORDER BY).
 */
import type {
  ConsolidationChoice,
  ConsolidationOutcome,
  ConsolidationPlan,
  MemoryAddResult,
  MemoryFact,
} from '../tauri-api';

let facts: MemoryFact[] = [];
let seq = 1;
let clock = 1_700_000_000; // монотонные «секунды» (без Date — детерминизм в тестах)
let opGroupSeq = 1;

/** MEM-8c-b: лог откатываемых операций по `opGroup` (мок не хранит события — держим обратимость явно). */
type UndoAction =
  | { kind: 'update'; factId: number; oldText: string; newText: string }
  | { kind: 'supersede'; restore: MemoryFact; newId: number; newText: string };
let undoLog = new Map<number, UndoAction>();

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

/** MEM-8: предложение консолидации. Мок БЕЗ эмбеддингов/LLM → находит только ТОЧНЫЙ дубль (→ `noop`),
 *  иначе `add` (как backend, когда нет близких выше порога). Честно: `update`/`supersede` в браузер-моке
 *  НЕ возникают — для них нужна семантика, которой здесь нет (preview consolidation ≈ обычный add). */
export async function consolidatePlan(
  text: string,
  source?: 'explicit' | 'auto',
): Promise<ConsolidationPlan> {
  const candidate = text.trim();
  const src = source ?? 'explicit';
  const dup = facts.find((f) => f.text === candidate);
  if (dup) return { candidate, source: src, op: { kind: 'noop', coveredBy: dup.id } };
  return { candidate, source: src, op: { kind: 'add' } };
}

/** MEM-8: применить выбор — зеркалит стейт-машину backend на in-memory списке (урок «Mock must match
 *  backend»). `supersede` = убрать target из ЖИВОГО списка (наблюдаемый эффект soft-supersede) + добавить
 *  новый; optimistic-деградация в add, если target изменился/исчез; `keepSeparate` всегда = add.
 *  ОГРАНИЧЕНИЕ (нет колонки `superseded_by`): soft-supersede смоделирован как ФИЗИЧЕСКОЕ удаление из
 *  списка → backend-путь «re-add супридённого текста → restore» здесь НЕ воспроизводится. В браузер-превью
 *  это недостижимо (нет эмбеддингов → `update`/`supersede`-планы не возникают); путь покрыт Rust-тестами. */
export async function consolidateApply(
  plan: ConsolidationPlan,
  choice: ConsolidationChoice,
): Promise<ConsolidationOutcome> {
  const cand = plan.candidate.trim();
  const op = plan.op;
  const addCandidate = (): ConsolidationOutcome => {
    const existing = facts.find((f) => f.text === cand);
    if (existing) return { op: 'add', id: existing.id, inserted: false }; // живой дубль
    const id = seq++;
    facts.push({
      id,
      text: cand,
      pinned: false,
      source: plan.source === 'auto' ? 'auto' : 'explicit',
      createdAt: clock++,
      usedAt: 0,
    });
    return { op: 'add', id, inserted: true };
  };

  if (op.kind === 'add') return addCandidate();
  if (op.kind === 'noop') return choice === 'keepSeparate' ? addCandidate() : { op: 'noop' };

  if (op.kind === 'update') {
    if (choice === 'keepSeparate') return addCandidate();
    const target = facts.find((f) => f.id === op.targetId);
    if (!target || target.text !== op.oldText) return addCandidate(); // optimistic-деградация
    if (op.newText === op.oldText) return { op: 'noop' }; // backend: правка без изменения → Noop (без события)
    const group = opGroupSeq++;
    undoLog.set(group, { kind: 'update', factId: target.id, oldText: op.oldText, newText: op.newText });
    target.text = op.newText;
    return { op: 'update', id: target.id, oldText: op.oldText, newText: op.newText, opGroup: group };
  }

  // supersede
  if (choice === 'keepSeparate') return addCandidate();
  const target = facts.find((f) => f.id === op.targetId);
  if (!target || target.text !== op.oldText) return addCandidate();
  const added = addCandidate();
  // Кандидат совпал с другим ЖИВЫМ фактом (не создан) → не супридим (backend-инвариант `!inserted`).
  if (added.op !== 'add' || !added.inserted) return added;
  const group = opGroupSeq++;
  const removed = facts.find((f) => f.id === op.targetId); // ещё в списке (addCandidate не трогал)
  if (removed) undoLog.set(group, { kind: 'supersede', restore: { ...removed }, newId: added.id, newText: cand });
  facts = facts.filter((f) => f.id !== op.targetId); // soft-supersede: target вне живого списка
  return {
    op: 'supersede',
    id: added.id,
    supersededId: op.targetId,
    oldText: op.oldText,
    newText: cand,
    inserted: true,
    opGroup: group,
  };
}

/** MEM-8c-b: откат группы — зеркалит `consolidate::undo` на observable-уровне (urok «Mock must match
 *  backend»). Реверсит по `undoLog`: update→вернуть текст; supersede→удалить новый + восстановить старый.
 *  Optimistic: реверсим только если состояние не правили после (текст нового/целевого == ожидаемый). */
export async function consolidateUndo(opGroup: number): Promise<boolean> {
  const a = undoLog.get(opGroup);
  if (!a) return false;
  undoLog.delete(opGroup);
  if (a.kind === 'update') {
    const f = facts.find((x) => x.id === a.factId);
    if (f && f.text === a.newText) f.text = a.oldText; // не правили после → откат
    return true;
  }
  // supersede: удалить новый (если не правили), восстановить старый
  const newF = facts.find((x) => x.id === a.newId);
  if (newF && newF.text === a.newText) facts = facts.filter((x) => x.id !== a.newId);
  if (!facts.some((x) => x.id === a.restore.id)) facts.push(a.restore);
  return true;
}

/** Сброс для тестов. */
export function __reset(): void {
  facts = [];
  seq = 1;
  clock = 1_700_000_000;
  opGroupSeq = 1;
  undoLog = new Map();
}
