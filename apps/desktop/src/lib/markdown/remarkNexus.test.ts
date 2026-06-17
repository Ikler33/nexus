import { describe, expect, it } from 'vitest';

import { splitWikilinksTags, wikilinkTarget } from './remarkNexus';

describe('remarkNexus splitter (#20)', () => {
  it('обычный текст → один text-узел', () => {
    const r = splitWikilinksTags('hello world');
    expect(r).toEqual([{ type: 'text', value: 'hello world' }]);
  });

  it('[[Target]] → link с nexus-схемой и подписью-целью', () => {
    const r = splitWikilinksTags('[[Target]]');
    expect(r).toHaveLength(1);
    expect(r[0]).toMatchObject({
      type: 'link',
      url: 'nexus-wikilink:Target',
      children: [{ type: 'text', value: 'Target' }],
    });
  });

  it('[[Target|Alias]] → подпись = alias; [[T#H]] → подпись без heading; target срезает |/#', () => {
    expect(splitWikilinksTags('[[A|Alias]]')[0]).toMatchObject({
      url: 'nexus-wikilink:A',
      children: [{ type: 'text', value: 'Alias' }],
    });
    expect(splitWikilinksTags('[[A#H]]')[0]).toMatchObject({
      url: 'nexus-wikilink:A',
      children: [{ type: 'text', value: 'A' }],
    });
    expect(wikilinkTarget('A#H')).toBe('A');
    expect(wikilinkTarget('A|B')).toBe('A');
  });

  it('#tag → tag-link, граница сохраняется как текст', () => {
    const r = splitWikilinksTags('see #idea here');
    // ['see', ' ', tag(#idea), ' here']
    const tag = r.find((n) => n.type === 'link');
    expect(tag).toMatchObject({
      type: 'link',
      url: 'nexus-tag:idea',
      children: [{ type: 'text', value: '#idea' }],
    });
    const flat = r.map((n) =>
      n.type === 'text' ? n.value : n.type === 'link' && n.children[0]?.type === 'text' ? n.children[0].value : '',
    );
    expect(flat.join('')).toBe('see #idea here');
  });

  it('смешанный текст: вики-ссылка + тег + текст вокруг', () => {
    const r = splitWikilinksTags('go [[Note]] then #done');
    const links = r.filter((n) => n.type === 'link');
    expect(links).toHaveLength(2);
    expect(links[0]).toMatchObject({ url: 'nexus-wikilink:Note' });
    expect(links[1]).toMatchObject({ url: 'nexus-tag:done' });
  });

  it('кириллический #тег кликабелен (зеркалит Unicode is_tag_char бэкенда)', () => {
    const r = splitWikilinksTags('запиши #идея сюда');
    const tag = r.find((n) => n.type === 'link');
    expect(tag).toMatchObject({
      type: 'link',
      url: `nexus-tag:${encodeURIComponent('идея')}`,
      children: [{ type: 'text', value: '#идея' }],
    });
  });

  it('вложенный тег с / (#проект/идея) — буквы Unicode + слэш', () => {
    const r = splitWikilinksTags('#проект/идея');
    expect(r.find((n) => n.type === 'link')).toMatchObject({
      url: `nexus-tag:${encodeURIComponent('проект/идея')}`,
    });
  });

  it('#123 (только цифры) НЕ тег (нужна хотя бы буква, как бэкенд)', () => {
    const r = splitWikilinksTags('число #123 тут');
    expect(r.every((n) => n.type === 'text')).toBe(true);
  });
});
