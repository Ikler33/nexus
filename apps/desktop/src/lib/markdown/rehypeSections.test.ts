import type { Element, Properties, Root } from 'hast';
import { describe, expect, it } from 'vitest';

import { rehypeSections } from './rehypeSections';

/** Хелпер: element-узел с плоским текстом. */
function el(tagName: string, text = '', props: Properties = {}): Element {
  return {
    type: 'element',
    tagName,
    properties: props,
    children: text ? [{ type: 'text', value: text }] : [],
  };
}

/** Прогнать плагин над root c заданными детьми; вернуть мутированный root. */
function run(children: Root['children']): Root {
  const tree: Root = { type: 'root', children };
  rehypeSections()(tree);
  return tree;
}

/** Узлы контента тела секции: section → [h2, .sec-body[.sec-inner[…]]] → дети sec-inner. */
function innerNodes(section: Element): Element[] {
  const body = section.children[1] as Element; // .sec-body
  const inner = body.children[0] as Element; // .sec-inner (единственный ребёнок)
  return inner.children as Element[];
}

/** GFM-блок сносок для тестов. */
function footnotes(): Element {
  return {
    type: 'element',
    tagName: 'section',
    properties: { dataFootnotes: true, className: ['footnotes'] },
    children: [{ type: 'element', tagName: 'ol', properties: {}, children: [] }],
  };
}

describe('rehypeSections (S3): группировка H2-секций', () => {
  it('h2 + сиблинги → <section.sec data-sec-id>[h2, .sec-body[.sec-inner[…]]] (grid-rows-обёртка)', () => {
    const root = run([el('h2', 'Раздел'), el('p', 'тело'), el('ul')]);
    expect(root.children).toHaveLength(1);
    const section = root.children[0] as Element;
    expect(section.tagName).toBe('section');
    expect(section.properties?.className).toEqual(['sec']);
    expect(section.properties?.['data-sec-id']).toBe('раздел');
    // первый ребёнок секции — САМ h2 (не копия), второй — .sec-body c ЕДИНСТВЕННЫМ ребёнком .sec-inner
    const [h2, body] = section.children as Element[];
    expect(h2.tagName).toBe('h2');
    expect(body.tagName).toBe('div');
    expect(body.properties?.className).toEqual(['sec-body']);
    expect(body.children).toHaveLength(1); // grid c одним треком → ровно один ребёнок
    const inner = body.children[0] as Element;
    expect(inner.tagName).toBe('div');
    expect(inner.properties?.className).toEqual(['sec-inner']);
    expect((inner.children as Element[]).map((c) => c.tagName)).toEqual(['p', 'ul']);
  });

  it('лид/интро ДО первого h2 остаётся ВНЕ секций (плоско)', () => {
    const root = run([el('p', 'лид'), el('h2', 'Первая'), el('p', 'тело')]);
    expect(root.children).toHaveLength(2);
    expect((root.children[0] as Element).tagName).toBe('p'); // лид вне секции
    expect((root.children[1] as Element).tagName).toBe('section');
  });

  it('несколько h2 → несколько секций, каждая до следующего h2', () => {
    const root = run([
      el('h2', 'A'),
      el('p', 'a-body'),
      el('h2', 'B'),
      el('p', 'b-body'),
      el('p', 'b-more'),
    ]);
    expect(root.children).toHaveLength(2);
    const [s1, s2] = root.children as Element[];
    expect(s1.properties?.['data-sec-id']).toBe('a');
    expect(s2.properties?.['data-sec-id']).toBe('b');
    expect(innerNodes(s2).length).toBe(2); // b-body + b-more
  });

  it('h3 НЕ создаёт секцию — живёт внутри .sec-inner своего h2', () => {
    const root = run([el('h2', 'Top'), el('h3', 'Под'), el('p', 'x')]);
    expect(root.children).toHaveLength(1);
    expect(innerNodes(root.children[0] as Element).map((c) => c.tagName)).toEqual(['h3', 'p']);
  });

  it('документ без h2 → НЕ падает, поток отдаётся как есть (нет секций)', () => {
    const root = run([el('p', 'a'), el('ul'), el('h3', 'h3 не top-level')]);
    expect(root.children).toHaveLength(3);
    expect(root.children.every((c) => (c as Element).tagName !== 'section')).toBe(true);
  });

  it('пустой документ → пустой (не падает)', () => {
    const root = run([]);
    expect(root.children).toHaveLength(0);
  });

  it('одноимённые секции дедуплицируются (data-sec-id: slug, slug-1)', () => {
    const root = run([el('h2', 'Обзор'), el('p'), el('h2', 'Обзор'), el('p')]);
    const [s1, s2] = root.children as Element[];
    expect(s1.properties?.['data-sec-id']).toBe('обзор');
    expect(s2.properties?.['data-sec-id']).toBe('обзор-1');
  });

  it('h2 из одной пунктуации (пустой slug) → стабильный fallback `section`', () => {
    const root = run([el('h2', '!!!'), el('p')]);
    expect((root.children[0] as Element).properties?.['data-sec-id']).toBe('section');
  });

  it('h2-узел ПЕРЕМЕЩАЕТСЯ (та же ссылка) — не копируется (HEADANCHOR-1: оверрайд h2 сработает)', () => {
    const h2 = el('h2', 'Раздел');
    const root = run([h2, el('p')]);
    const section = root.children[0] as Element;
    expect(section.children[0]).toBe(h2); // идентичность сохранена (move, not copy)
  });

  it('GFM-блок сносок НЕ всасывается в тело последней секции — выносится top-level', () => {
    const fn = footnotes();
    const root = run([el('h2', 'Раздел'), el('p', 'тело'), fn]);
    expect(root.children).toHaveLength(2); // [section, footnotes]
    const section = root.children[0] as Element;
    expect(section.tagName).toBe('section');
    // тело секции — только `p`, БЕЗ footnotes
    expect(innerNodes(section).map((c) => c.tagName)).toEqual(['p']);
    // footnotes-блок — отдельно, top-level (та же ссылка), виден при сворачивании секции
    expect(root.children[1]).toBe(fn);
  });

  it('хвост ПОСЛЕ footnotes тоже выносится наружу (footnotes не дробит на под-секции)', () => {
    const fn = footnotes();
    const trailing = el('p', 'хвост');
    const root = run([el('h2', 'A'), el('p'), fn, trailing]);
    // [section A, footnotes, trailing] — секций после footnotes не нарезаем
    expect(root.children).toHaveLength(3);
    expect((root.children[0] as Element).tagName).toBe('section');
    expect(root.children[1]).toBe(fn);
    expect(root.children[2]).toBe(trailing);
  });
});
