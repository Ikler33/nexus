import { describe, expect, it } from 'vitest';
import { parseInbox, removeLine } from './parse';

describe('parseInbox (INBOX-1)', () => {
  it('извлекает строки `- HH:MM текст` с 1-based номерами', () => {
    const doc = '# Inbox\n- 09:05 позвонить маме\n- 14:30 купить хлеб';
    expect(parseInbox(doc)).toEqual([
      { line: 2, time: '09:05', text: 'позвонить маме' },
      { line: 3, time: '14:30', text: 'купить хлеб' },
    ]);
  });

  it('игнорирует заголовок и не-захват-строки', () => {
    expect(parseInbox('# Inbox\nпросто текст\n- без времени')).toEqual([]);
  });

  it('CRLF: завершающий \\r не попадает в текст', () => {
    expect(parseInbox('- 09:00 дело\r\n')).toEqual([{ line: 1, time: '09:00', text: 'дело' }]);
  });

  it('пустой документ → []', () => {
    expect(parseInbox('')).toEqual([]);
  });
});

describe('removeLine (INBOX-1)', () => {
  it('вырезает указанную 1-based строку', () => {
    expect(removeLine('a\nb\nc', 2)).toBe('a\nc');
  });

  it('вне диапазона → null (дрейф)', () => {
    expect(removeLine('a\nb', 0)).toBeNull();
    expect(removeLine('a\nb', 3)).toBeNull();
  });
});
