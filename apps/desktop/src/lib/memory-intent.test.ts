import { describe, expect, it } from 'vitest';

import { isExplicitSave, stripSaveCommand } from './memory-intent';

describe('isExplicitSave — явная команда сохранить в память (MEM-5)', () => {
  it('распознаёт явные RU-команды', () => {
    expect(isExplicitSave('Сохрани в памяти то что я работаю над RMS B2B')).toBe(true);
    expect(isExplicitSave('запомни: дедлайн в пятницу')).toBe(true);
    expect(isExplicitSave('Запомните, что я пишу на Rust')).toBe(true);
    expect(isExplicitSave('добавь в память: мой проект — Nexus')).toBe(true);
    expect(isExplicitSave('занеси в память что я из Тбилиси')).toBe(true);
  });

  it('распознаёт явные EN-команды', () => {
    expect(isExplicitSave('remember that I prefer dark theme')).toBe(true);
    expect(isExplicitSave('save this to memory')).toBe(true);
    expect(isExplicitSave('keep in mind that the demo is Friday')).toBe(true);
  });

  it('НЕ срабатывает на обычных вопросах и упоминаниях памяти', () => {
    expect(isExplicitSave('А ты помнишь что я говорил про проект?')).toBe(false);
    expect(isExplicitSave('сколько у тебя памяти?')).toBe(false);
    expect(isExplicitSave('расскажи про RMS B2B')).toBe(false);
    expect(isExplicitSave('')).toBe(false);
    expect(isExplicitSave('   ')).toBe(false);
  });

  it('НЕ путает «запоминай» (впредь) с «запомни» (это)', () => {
    expect(isExplicitSave('запоминай мои предпочтения по ходу')).toBe(false);
  });

  it('уважает отрицание (и перфективные «не запомни/сохрани» — ревью MEM-5)', () => {
    expect(isExplicitSave('не запоминай это, это шутка')).toBe(false);
    expect(isExplicitSave('не запомни это')).toBe(false); // перфектив, не путать с командой
    expect(isExplicitSave('не сохрани это в память, отмена')).toBe(false);
    expect(isExplicitSave("don't remember that")).toBe(false);
    expect(isExplicitSave("don't save this to memory")).toBe(false);
  });
});

describe('stripSaveCommand — фолбэк-срез команды (MEM-5)', () => {
  it('срезает кириллический командный префикс (без `\\b`-ASCII-бага)', () => {
    expect(stripSaveCommand('запомни что мой дедлайн в пятницу')).toBe('мой дедлайн в пятницу');
    expect(stripSaveCommand('Сохрани в памяти то что я работаю над RMS B2B')).toBe(
      'я работаю над RMS B2B',
    );
    expect(stripSaveCommand('добавь в память: проект Nexus')).toBe('проект Nexus');
  });

  it('срезает EN-префикс', () => {
    expect(stripSaveCommand('remember that I prefer dark theme')).toBe('I prefer dark theme');
    expect(stripSaveCommand('save to memory: deadline Friday')).toBe('deadline Friday');
  });

  it('без команды — возвращает как есть; режет до 140 символов', () => {
    expect(stripSaveCommand('я работаю над проектом')).toBe('я работаю над проектом');
    expect(stripSaveCommand('запомни ' + 'a'.repeat(200)).length).toBeLessThanOrEqual(140);
  });
});
