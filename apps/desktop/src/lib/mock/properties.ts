// Мок реестра типов свойств (PROP-2): зеркалит контракт Rust `properties` — реестр явных типов +
// эвристика по значению (MEM-5: мок должен совпадать с бэкендом). `forNote` отдаёт плоские
// frontmatter-скаляры с разрешённым типом (как `get_note_properties`).

import type { NoteProperty, PropertyType } from '../tauri-api';

/** Имена, форсящие тип Tags (Obsidian). */
const FORCED_TAGS = new Set(['tags', 'aliases', 'cssclasses']);

const isBool = (v: string) =>
  ['true', 'false', 'yes', 'no', 'on', 'off'].includes(v.toLowerCase());
const isIsoDate = (v: string) => /^\d{4}-\d{2}-\d{2}$/.test(v);
const isIsoDatetime = (v: string) => /^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}/.test(v);
const isNumber = (v: string) => v !== '' && Number.isFinite(Number(v));
const isInlineList = (v: string) => v.startsWith('[') && v.endsWith(']');

/** Эвристика типа по значению (зеркало Rust `infer_type`): порядок forced→bool→datetime→date→number→list→text. */
export function inferType(key: string, value: string): PropertyType {
  if (FORCED_TAGS.has(key.toLowerCase())) return 'tags';
  const v = value.trim();
  if (isBool(v)) return 'checkbox';
  if (isIsoDatetime(v)) return 'datetime';
  if (isIsoDate(v)) return 'date';
  if (isNumber(v)) return 'number';
  if (isInlineList(v)) return 'list';
  return 'text';
}

// Мутабельный реестр явных типов (переживает в рамках сессии, как бэкенд-файл).
let REGISTRY: Record<string, PropertyType> = {};

export async function types(): Promise<Record<string, PropertyType>> {
  return { ...REGISTRY };
}

export async function setType(key: string, type: PropertyType): Promise<void> {
  REGISTRY = { ...REGISTRY, [key.trim()]: type };
}

/** Свойства заметки (зеркало `get_note_properties`): репрезентативные frontmatter-скаляры + тип. */
export async function forNote(): Promise<NoteProperty[]> {
  const fields: [string, string][] = [
    ['status', 'doing'],
    ['priority', 'high'],
    ['due', '2026-06-20'],
    ['created', '2026-06-16'],
  ];
  return fields.map(([key, value]) => ({ key, value, type: REGISTRY[key] ?? inferType(key, value) }));
}
