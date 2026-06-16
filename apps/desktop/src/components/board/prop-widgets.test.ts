import { describe, expect, it } from 'vitest';

import { isChecked, isValidForType } from './prop-widgets';

describe('prop-widgets (PROP-3)', () => {
  it('isValidForType по типам (строгость ≥ виджет/бэкенд, ревью R1–R4)', () => {
    expect(isValidForType('text', 'что угодно')).toBe(true);
    // number — десятичное/экспоненциальное; 0x/0b/Infinity отвергаются (native input их не покажет).
    expect(isValidForType('number', '3.5')).toBe(true);
    expect(isValidForType('number', '1e5')).toBe(true);
    expect(isValidForType('number', 'abc')).toBe(false);
    expect(isValidForType('number', '0x10')).toBe(false);
    expect(isValidForType('number', 'Infinity')).toBe(false);
    expect(isValidForType('checkbox', 'true')).toBe(true);
    expect(isValidForType('checkbox', 'maybe')).toBe(false);
    // date — КАЛЕНДАРНАЯ валидность, не только форма (R1).
    expect(isValidForType('date', '2026-06-20')).toBe(true);
    expect(isValidForType('date', '2026-02-30')).toBe(false); // форма ок, календарь нет
    expect(isValidForType('date', '20.06.2026')).toBe(false);
    // datetime/list/tags редактируются как текст / read-only → любое значение «ок» (R2/R4).
    expect(isValidForType('datetime', 'что угодно')).toBe(true);
    expect(isValidForType('list', 'Иван')).toBe(true);
    expect(isValidForType('tags', 'just text')).toBe(true);
  });

  it('isChecked распознаёт truthy bool', () => {
    expect(isChecked('true')).toBe(true);
    expect(isChecked('Yes')).toBe(true);
    expect(isChecked('on')).toBe(true);
    expect(isChecked('false')).toBe(false);
    expect(isChecked('off')).toBe(false);
  });
});
