import { describe, expect, it } from 'vitest';

import { tagCompletionQuery } from './tag-complete';

describe('tagCompletionQuery (PROP-4 — контекст автокомплита тегов, §14.5)', () => {
  it('инлайн #tag после пробела/начала строки', () => {
    expect(tagCompletionQuery('#иде')).toBe('иде'); // начало строки, кириллица
    expect(tagCompletionQuery('текст #proj')).toBe('proj');
    expect(tagCompletionQuery('текст #a/b')).toBe('a/b'); // вложенность
    expect(tagCompletionQuery('#')).toBe(''); // только что ввели #
  });

  it('заголовок `# ` НЕ автокомплит (после # пробел)', () => {
    expect(tagCompletionQuery('# Заголовок')).toBeNull();
    expect(tagCompletionQuery('# ')).toBeNull();
    // но тег ВНУТРИ строки-заголовка — ок
    expect(tagCompletionQuery('# Заголовок #tag')).toBe('tag');
  });

  it('не автокомплит в инлайн-code-span (нечётные бэктики)', () => {
    expect(tagCompletionQuery('`#tag')).toBeNull();
    expect(tagCompletionQuery('`code` #tag')).toBe('tag'); // чётное число ` → не в коде
  });

  it('frontmatter tags: инлайн-список', () => {
    expect(tagCompletionQuery('tags: [proj')).toBe('proj');
    expect(tagCompletionQuery('tags: [task, fr')).toBe('fr');
    expect(tagCompletionQuery('aliases: [al')).toBe('al');
  });

  it('не тег-контекст → null', () => {
    expect(tagCompletionQuery('просто текст')).toBeNull();
    expect(tagCompletionQuery('email@host')).toBeNull(); // # не нужен
    expect(tagCompletionQuery('a#b')).toBeNull(); // # в середине слова (без пробела)
  });
});
