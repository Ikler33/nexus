import { describe, expect, it } from 'vitest';
import { cleanHeadingText, extractHeadings } from './outline';

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
