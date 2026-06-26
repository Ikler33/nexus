import { describe, expect, it } from 'vitest';
import { cleanHeadingText, extractHeadings, pickActiveLine } from './outline';

describe('pickActiveLine (Hermes-8 S6 scroll-spy)', () => {
  // top — расстояние верха заголовка от верха вьюпорта; threshold = 90 (README §6).
  it('активна ПОСЛЕДНЯЯ секция, чей top ≤ порога', () => {
    const heads = [
      { line: 1, top: -120 }, // уехал вверх за фолд
      { line: 5, top: 40 }, // в пределах порога (≤90)
      { line: 9, top: 200 }, // ниже порога
    ];
    expect(pickActiveLine(heads, 90)).toBe(5);
  });

  it('скролл выше первого заголовка (все top > порога) → подсвечивается ПЕРВЫЙ (подсветка не гаснет)', () => {
    const heads = [
      { line: 2, top: 150 },
      { line: 6, top: 320 },
    ];
    expect(pickActiveLine(heads, 90)).toBe(2);
  });

  it('граница: top ровно = порогу включается (≤, не <)', () => {
    expect(pickActiveLine([{ line: 3, top: 90 }], 90)).toBe(3);
  });

  it('пусто → null (нет заголовков)', () => {
    expect(pickActiveLine([], 90)).toBeNull();
  });

  it('все заголовки выше фолда → активен последний (самый нижний прошедший порог)', () => {
    const heads = [
      { line: 1, top: -300 },
      { line: 4, top: -120 },
      { line: 8, top: -10 },
    ];
    expect(pickActiveLine(heads, 90)).toBe(8);
  });
});

describe('extractHeadings (EDIT-7)', () => {
  it('извлекает ATX-заголовки с уровнем и 1-based номером строки', () => {
    const doc = ['# Title', '', 'текст', '## Section', '### Sub'].join('\n');
    expect(extractHeadings(doc)).toEqual([
      { level: 1, text: 'Title', line: 1 },
      { level: 2, text: 'Section', line: 4 },
      { level: 3, text: 'Sub', line: 5 },
    ]);
  });

  it('игнорирует `#` внутри огороженных код-блоков (``` и ~~~)', () => {
    const doc = [
      '# Real',
      '```bash',
      '# не заголовок (комментарий)',
      '```',
      '## After',
      '~~~',
      '### тоже не заголовок',
      '~~~',
      '#### Last',
    ].join('\n');
    expect(extractHeadings(doc).map((h) => h.text)).toEqual(['Real', 'After', 'Last']);
  });

  it('не принимает `#` без пробела за хеш-заголовок (`#tag`, `C#`)', () => {
    const doc = ['#tag не заголовок', '# C# basics', 'C# в тексте'].join('\n');
    const hs = extractHeadings(doc);
    expect(hs).toHaveLength(1);
    expect(hs[0]).toEqual({ level: 1, text: 'C# basics', line: 2 });
  });

  it('срезает закрывающую последовательность решёток только после пробела', () => {
    expect(extractHeadings('# Heading ###')[0].text).toBe('Heading');
    expect(extractHeadings('# foo#')[0].text).toBe('foo#'); // нет пробела — `#` часть текста
  });

  it('уважает отступ ≤3 пробела, но не глубже', () => {
    expect(extractHeadings('   ### ok')[0]).toEqual({ level: 3, text: 'ok', line: 1 });
    expect(extractHeadings('    # too deep (код)')).toEqual([]);
  });

  it('пропускает заголовки с пустым текстом (только решётки/пробелы)', () => {
    expect(extractHeadings(['# ', '##   ', '# Real'].join('\n'))).toEqual([
      { level: 1, text: 'Real', line: 3 },
    ]);
  });

  it('срезает эмодзи из заголовков (совпадает с рендером remarkStripHeadingEmoji)', () => {
    const doc = ['# 📅 2026-03-05', '## 🧠 Поток мыслей', '## 💡 Идеи'].join('\n');
    expect(extractHeadings(doc)).toEqual([
      { level: 1, text: '2026-03-05', line: 1 },
      { level: 2, text: 'Поток мыслей', line: 2 },
      { level: 2, text: 'Идеи', line: 3 },
    ]);
  });

  it('пустой документ → пустой список', () => {
    expect(extractHeadings('')).toEqual([]);
  });
});

describe('cleanHeadingText (EDIT-7)', () => {
  it('снимает инлайн-разметку для читаемого показа', () => {
    expect(cleanHeadingText('**Bold** and *italic*')).toBe('Bold and italic');
    expect(cleanHeadingText('`code` block')).toBe('code block');
    expect(cleanHeadingText('~~strike~~ and ==mark==')).toBe('strike and mark');
  });

  it('разворачивает ссылки и викилинки в их текст', () => {
    expect(cleanHeadingText('[читать](https://x.io)')).toBe('читать');
    expect(cleanHeadingText('[[Note]]')).toBe('Note');
    expect(cleanHeadingText('[[Note|алиас]]')).toBe('алиас');
  });

  it('не трогает внутрисловные `_` (CommonMark: snake_case не эмфазис)', () => {
    expect(cleanHeadingText('snake_case_name')).toBe('snake_case_name');
    expect(cleanHeadingText('foo_bar_baz')).toBe('foo_bar_baz');
    // но `_слово_` на границах слова — снимается
    expect(cleanHeadingText('текст _курсив_ конец')).toBe('текст курсив конец');
  });
});
