import { describe, expect, it } from 'vitest';

import { removeHeadingEmoji, stripHeadingEmoji } from './headingText';

describe('stripHeadingEmoji (цельная строка: удаление + collapse + trim)', () => {
  it('срезает ведущий эмодзи и схлопывает зазор (шаблон daily)', () => {
    expect(stripHeadingEmoji('🧠 Поток мыслей')).toBe('Поток мыслей');
    expect(stripHeadingEmoji('💡 Идеи')).toBe('Идеи');
    expect(stripHeadingEmoji('📅 2026-03-05')).toBe('2026-03-05');
  });

  it('срезает supplementary-эмодзи в середине/конце, схлопывая двойные пробелы', () => {
    expect(stripHeadingEmoji('Заметка 🧠 о мозге')).toBe('Заметка о мозге');
    expect(stripHeadingEmoji('Готово 🎉')).toBe('Готово'); // 🎉 U+1F389 — supplementary
  });

  it('убирает эмодзи-последовательности (флаги, ZWJ, variation selector)', () => {
    expect(stripHeadingEmoji('🇷🇺 Россия')).toBe('Россия'); // региональные индикаторы
    expect(stripHeadingEmoji('❤️ Любовь')).toBe('Любовь'); // VS-16
    expect(stripHeadingEmoji('👨‍👩‍👧 Семья')).toBe('Семья'); // ZWJ-последовательность
  });

  // adversarial FIX 2 (MAJOR): text-презентационные символы НЕ режутся.
  it('НЕ трогает text-символы ™ © ® ℹ ↗ ✔ ⚙ ▶ (легитимный текст, не декор)', () => {
    expect(stripHeadingEmoji('Acme™')).toBe('Acme™');
    expect(stripHeadingEmoji('© 2026 ®')).toBe('© 2026 ®');
    expect(stripHeadingEmoji('ℹ инфо ↗ ✔ ⚙ ▶')).toBe('ℹ инфо ↗ ✔ ⚙ ▶');
  });

  it('режет символ ТОЛЬКО с emoji-вариантом VS-16: `⚙️`→∅, голый `⚙` цел', () => {
    expect(stripHeadingEmoji('⚙️ Настройки')).toBe('Настройки');
    expect(stripHeadingEmoji('⚙ Настройки')).toBe('⚙ Настройки');
  });

  it('НЕ трогает буквы/цифры/пунктуацию/CJK/кириллицу', () => {
    expect(stripHeadingEmoji('Обычный заголовок')).toBe('Обычный заголовок');
    expect(stripHeadingEmoji('Plan #1: ROI 50% (Q3)')).toBe('Plan #1: ROI 50% (Q3)');
    expect(stripHeadingEmoji('日本語の見出し')).toBe('日本語の見出し');
    expect(stripHeadingEmoji('snake_case_name')).toBe('snake_case_name');
  });

  it('пустая/без-эмодзи строка возвращается как есть (после trim)', () => {
    expect(stripHeadingEmoji('')).toBe('');
    expect(stripHeadingEmoji('текст')).toBe('текст');
  });
});

describe('removeHeadingEmoji (поузельно-безопасное: БЕЗ trim/collapse)', () => {
  // adversarial FIX 1: для отдельных text-узлов заголовка границы НЕ трогаем — их чистит плагин.
  it('вырезает эмодзи, но НЕ триммит и НЕ схлопывает пробелы', () => {
    expect(removeHeadingEmoji('📅 До ')).toBe(' До '); // ведущий пробел СОХРАНЁН (не trim)
    expect(removeHeadingEmoji(' и B')).toBe(' и B'); // граничный пробел узла цел
    expect(removeHeadingEmoji('🧠')).toBe(''); // только эмодзи → пусто
  });

  it('те же FIX-2 границы: text-символы целы, VS-16-символ режется', () => {
    expect(removeHeadingEmoji('Acme™ ')).toBe('Acme™ ');
    expect(removeHeadingEmoji('⚙️ ')).toBe(' ');
    expect(removeHeadingEmoji('⚙ ')).toBe('⚙ ');
  });
});
