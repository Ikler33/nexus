import { describe, expect, it } from 'vitest';

import {
  EMBED_PARAGRAPH_RE,
  extractSection,
  isBlockAnchor,
  isImageTarget,
  parseEmbedTarget,
  parseImageParams,
} from './embed';

describe('parseEmbedTarget', () => {
  it('голая заметка', () => {
    expect(parseEmbedTarget('note')).toEqual({ note: 'note', anchor: null });
  });
  it('заметка#Заголовок', () => {
    expect(parseEmbedTarget('note#Heading')).toEqual({
      note: 'note',
      anchor: 'Heading',
    });
  });
  it('блок-ссылка #^id сохраняется как якорь с ^', () => {
    expect(parseEmbedTarget('note#^abc')).toEqual({
      note: 'note',
      anchor: '^abc',
    });
  });
  it('алиас |alias отбрасывается', () => {
    expect(parseEmbedTarget('note|alias')).toEqual({
      note: 'note',
      anchor: null,
    });
  });
  it('заголовок + алиас', () => {
    expect(parseEmbedTarget('note#H|alias')).toEqual({
      note: 'note',
      anchor: 'H',
    });
  });
  it('путь с папками', () => {
    expect(parseEmbedTarget('folder/sub/note#H')).toEqual({
      note: 'folder/sub/note',
      anchor: 'H',
    });
  });
  it('обрезает пробелы', () => {
    expect(parseEmbedTarget('  spaced #  H  ')).toEqual({
      note: 'spaced',
      anchor: 'H',
    });
  });
});

describe('isImageTarget', () => {
  it.each(['pic.png', 'a.JPEG', 'b.webp', 'c.svg', 'd.gif'])('%s → картинка', (t) => {
    expect(isImageTarget(t)).toBe(true);
  });
  it.each(['note', 'note.md', 'png', 'a.txt'])('%s → не картинка', (t) => {
    expect(isImageTarget(t)).toBe(false);
  });
});

describe('isBlockAnchor', () => {
  it('^id → блок', () => expect(isBlockAnchor('^abc')).toBe(true));
  it('заголовок → не блок', () => expect(isBlockAnchor('Heading')).toBe(false));
  it('null → не блок', () => expect(isBlockAnchor(null)).toBe(false));
});

describe('parseImageParams (![[img.png|alt|300]])', () => {
  it('без параметров', () => {
    expect(parseImageParams('img.png')).toEqual({ alt: '', width: null });
  });
  it('только ширина (число)', () => {
    expect(parseImageParams('img.png|300')).toEqual({ alt: '', width: 300 });
  });
  it('ШxВ → берём ширину', () => {
    expect(parseImageParams('img.png|200x100')).toEqual({ alt: '', width: 200 });
  });
  it('только alt (нечисло)', () => {
    expect(parseImageParams('img.png|схема')).toEqual({ alt: 'схема', width: null });
  });
  it('alt + ширина', () => {
    expect(parseImageParams('img.png|схема|250')).toEqual({ alt: 'схема', width: 250 });
  });
  it('ширина 0 → null (не делаем 0px-невидимую картинку, ревью)', () => {
    expect(parseImageParams('img.png|0')).toEqual({ alt: '', width: null });
  });
});

describe('extractSection', () => {
  const body = '# A\n\nalpha\n\n## Section\n\nbody line\n\n### Sub\n\nsub line\n\n## Other\n\nelse';

  it('секция уровня 2: от заголовка до следующего ≤ уровня (Sub включается, Other нет)', () => {
    expect(extractSection(body, 'Section')).toBe('## Section\n\nbody line\n\n### Sub\n\nsub line');
  });
  it('регистронезависимо', () => {
    expect(extractSection(body, 'section')).toContain('body line');
  });
  it('заголовок верхнего уровня тянет до конца, если ниже нет ≤ уровня', () => {
    expect(extractSection('## Only\n\nx\n\n### Deep\n\ny', 'Only')).toBe(
      '## Only\n\nx\n\n### Deep\n\ny',
    );
  });
  it('закрытый ATX `## H ##` матчится по H', () => {
    expect(extractSection('## H ##\n\nz', 'H')).toBe('## H ##\n\nz');
  });
  it('не найден → null', () => {
    expect(extractSection(body, 'Nope')).toBeNull();
  });
  it('пустой якорь → null', () => {
    expect(extractSection(body, '   ')).toBeNull();
  });

  // Ревью транклюзии: заголовок-подобная строка `# …` ВНУТРИ ```-фенса не должна рубить секцию.
  it('`# …` внутри код-фенса в теле секции НЕ обрывает её (фенс и хвост сохранены)', () => {
    const src = '## Real\n\nx\n\n```\n# Fake\n```\n\nmore real';
    expect(extractSection(src, 'Real')).toBe('## Real\n\nx\n\n```\n# Fake\n```\n\nmore real');
  });
  it('`# …` внутри фенса ПЕРЕД секцией не считается заголовком (старт по тексту верный)', () => {
    expect(extractSection('```\n# Fake\n```\n\n## Real\n\nz', 'Real')).toBe('## Real\n\nz');
  });
  it('заголовок, существующий только внутри фенса, не находится → null', () => {
    expect(extractSection('```\n# Fake\n```\n\nтекст', 'Fake')).toBeNull();
  });
  it('setext-заголовок (===/---) не поддержан (ATX-only, единообразно) → null', () => {
    expect(extractSection('Title\n===\n\nbody', 'Title')).toBeNull();
  });
});

describe('EMBED_PARAGRAPH_RE', () => {
  it('матчит ровно ![[ … ]]', () => {
    expect(EMBED_PARAGRAPH_RE.exec('![[Note]]')?.[1]).toBe('Note');
    expect(EMBED_PARAGRAPH_RE.exec('![[a/b#H|x]]')?.[1]).toBe('a/b#H|x');
  });
  it('НЕ матчит инлайн / многострочное / без !', () => {
    expect(EMBED_PARAGRAPH_RE.exec('text ![[X]] more')).toBeNull();
    expect(EMBED_PARAGRAPH_RE.exec('![[a]]\n![[b]]')).toBeNull();
    expect(EMBED_PARAGRAPH_RE.exec('[[X]]')).toBeNull();
  });
});
