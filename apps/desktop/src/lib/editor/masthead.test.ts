import { describe, expect, it } from 'vitest';

import { basenameTitle, deriveMasthead, dropCapLetter } from './masthead';

describe('dropCapLetter', () => {
  it('берёт первую букву в верхнем регистре', () => {
    expect(dropCapLetter('много текста')).toBe('М');
    expect(dropCapLetter('the quick brown')).toBe('T');
  });
  it('пропускает ведущие пробелы/символы до первой буквы', () => {
    expect(dropCapLetter('  «слово»')).toBe('С');
    expect(dropCapLetter('— тире')).toBe('Т');
  });
  it('пусто, если букв нет', () => {
    expect(dropCapLetter('123 456')).toBe('');
    expect(dropCapLetter('   ')).toBe('');
    expect(dropCapLetter('')).toBe('');
  });
});

describe('basenameTitle', () => {
  it('срезает каталог и расширение', () => {
    expect(basenameTitle('Projects/Nexus/Идея.md')).toBe('Идея');
    expect(basenameTitle('README.markdown')).toBe('README');
    expect(basenameTitle('заметка')).toBe('заметка');
  });
  it('пусто для undefined', () => {
    expect(basenameTitle(undefined)).toBe('');
  });
});

describe('deriveMasthead — заголовок', () => {
  it('frontmatter title имеет приоритет над H1 и именем файла', () => {
    const src = '---\ntitle: Из фронтматтера\n---\n# H1 заголовок\nтекст';
    const m = deriveMasthead(src, 'file.md');
    expect(m.title).toBe('Из фронтматтера');
  });
  it('текст ведущего H1, если нет frontmatter title', () => {
    const m = deriveMasthead('# Настоящий заголовок\n\nтекст', 'file.md');
    expect(m.title).toBe('Настоящий заголовок');
    expect(m.h1Line).toBe(1);
  });
  it('имя файла, если нет ни title, ни H1', () => {
    const m = deriveMasthead('просто текст без заголовка', 'Папка/Моя заметка.md');
    expect(m.title).toBe('Моя заметка');
    expect(m.h1Line).toBeNull();
  });
  it('снимает закрывающую ATX-последовательность (# … #), но не # без пробела', () => {
    expect(deriveMasthead('# Заголовок #\nтекст', 'f.md').title).toBe('Заголовок');
    expect(deriveMasthead('# Заголовок ###\nтекст', 'f.md').title).toBe('Заголовок');
    expect(deriveMasthead('# Цена 5#\nтекст', 'f.md').title).toBe('Цена 5#'); // нет пробела → не закрытие
  });
  it('снимает inline-маркеры * и ` из отображаемого заголовка, но не из h1Text (для slug)', () => {
    const m = deriveMasthead('# Идея **важная** и `код`\nтекст', 'f.md');
    expect(m.title).toBe('Идея важная и код');
    expect(m.h1Text).toBe('Идея **важная** и `код`'); // сырой — для slugify
  });
  it('h1Text null, если ведущего H1 нет', () => {
    expect(deriveMasthead('текст без H1', 'f.md').h1Text).toBeNull();
  });
});

describe('deriveMasthead — kicker (теги)', () => {
  it('собирает теги из frontmatter, снимает ведущий #', () => {
    const m = deriveMasthead('---\ntags: [project, "#ai"]\n---\nтекст', 'f.md');
    expect(m.tags).toEqual(['project', 'ai']);
  });
  it('поддерживает блок-список тегов', () => {
    const m = deriveMasthead('---\ntags:\n  - one\n  - two\n---\nтекст', 'f.md');
    expect(m.tags).toEqual(['one', 'two']);
  });
  it('нет тегов → пустой массив', () => {
    expect(deriveMasthead('# H\nтекст', 'f.md').tags).toEqual([]);
  });
});

describe('deriveMasthead — body (обнуление H1 сохраняет номера строк)', () => {
  it('обнуляет строку H1, не удаляя её — номера строк ниже не сдвигаются', () => {
    const src = '# Заголовок\n- [ ] задача';
    const m = deriveMasthead(src, 'f.md');
    const lines = m.body.split('\n');
    expect(lines.length).toBe(src.split('\n').length); // строк столько же
    expect(lines[0]).toBe(''); // H1 обнулён
    expect(lines[1]).toBe('- [ ] задача'); // задача осталась на 2-й строке (1-based 2)
  });
  it('H1 после frontmatter: обнуляется правильная строка тела', () => {
    const src = '---\nstatus: doing\n---\n# Заголовок\nтекст';
    const m = deriveMasthead(src, 'f.md');
    const lines = m.body.split('\n');
    expect(lines[3]).toBe(''); // H1 был 4-й строкой (после ---,status,---)
    expect(lines[4]).toBe('текст');
    expect(m.h1Line).toBe(4);
  });
  it('нет ведущего H1 → тело без изменений', () => {
    const src = '## Подзаголовок\nтекст';
    const m = deriveMasthead(src, 'f.md');
    expect(m.body).toBe(src);
    expect(m.h1Line).toBeNull();
  });
  it('#tag без пробела не считается H1', () => {
    const src = '#tag в начале';
    const m = deriveMasthead(src, 'f.md');
    expect(m.body).toBe(src);
    expect(m.h1Line).toBeNull();
  });
});

describe('deriveMasthead — поля для Properties (title/tags вынесены в масthead)', () => {
  it('убирает title/tags, оставляет прочие поля', () => {
    const src = '---\ntitle: T\ntags: [a]\nstatus: doing\npriority: high\n---\nтекст';
    const m = deriveMasthead(src, 'f.md');
    expect(m.fields.map((f) => f.key)).toEqual(['status', 'priority']);
  });
  it('нет frontmatter → пустые поля', () => {
    expect(deriveMasthead('# H\nтекст', 'f.md').fields).toEqual([]);
  });
});
