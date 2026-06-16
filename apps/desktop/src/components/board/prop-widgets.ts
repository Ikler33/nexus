// Чистые хелперы Properties-виджетов (PROP-3): валидация «значение под типом?» (иначе invalid → править в
// source, §7) + bool чекбокса. Без React — юнит-тестируемо. Зеркалит эвристику бэкенда (`properties`).

import type { PropertyType } from '../../lib/tauri-api';

const BOOL_TRUE = ['true', 'yes', 'on'];
const BOOL_ALL = ['true', 'false', 'yes', 'no', 'on', 'off'];
// Десятичное/экспоненциальное число (БЕЗ `0x`/`0b`/`Infinity` — их `<input type=number>` не покажет).
const NUMBER_RE = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;

/** Валидная КАЛЕНДАРНАЯ дата `YYYY-MM-DD` (не только форма — `2026-02-30` отвергается; иначе native
 *  date-input показал бы пусто → ложная ошибка при blur, ревью R1). */
export function isCalendarDate(v: string): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(v.trim());
  if (!m) return false;
  const y = +m[1];
  const mo = +m[2];
  const d = +m[3];
  const dt = new Date(Date.UTC(y, mo - 1, d));
  return dt.getUTCFullYear() === y && dt.getUTCMonth() === mo - 1 && dt.getUTCDate() === d;
}

/**
 * Соответствует ли значение типу — выбор ТИПИЗИРОВАННОГО виджета (true) vs жёлтое «invalid»-поле (false).
 * Строгость должна быть НЕ слабее native-виджета И бэкенд-round-trip (ревью R1–R4): иначе значение,
 * которое надо отправить в «Править в source», попадает в виджет, который не может его показать/сохранить.
 * text/datetime/list/tags редактируются как текст / read-only (list/tags — PROP-4) → любое значение «ок».
 */
export function isValidForType(type: PropertyType, value: string): boolean {
  const v = value.trim();
  switch (type) {
    case 'text':
    case 'datetime':
    case 'list':
    case 'tags':
      return true;
    case 'number':
      return NUMBER_RE.test(v);
    case 'checkbox':
      return BOOL_ALL.includes(v.toLowerCase());
    case 'date':
      return isCalendarDate(v);
  }
}

/** Bool-состояние чекбокса из строкового значения. */
export function isChecked(value: string): boolean {
  return BOOL_TRUE.includes(value.trim().toLowerCase());
}
