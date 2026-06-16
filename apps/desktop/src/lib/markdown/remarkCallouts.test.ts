import type { Blockquote, Paragraph, Root } from 'mdast';
import { describe, expect, it } from 'vitest';

import { parseCalloutMarker, remarkCallouts, splitInlineAtNewline } from './remarkCallouts';

describe('parseCalloutMarker', () => {
  it('голый маркер без заголовка', () => {
    expect(parseCalloutMarker('[!note]')).toEqual({
      marker: { kind: 'note', fold: '', rawLabel: 'note' },
      rest: '',
    });
  });
  it('маркер + заголовок', () => {
    expect(parseCalloutMarker('[!warning] Осторожно')).toEqual({
      marker: { kind: 'warning', fold: '', rawLabel: 'warning' },
      rest: 'Осторожно',
    });
  });
  it('нижний регистр типа, rawLabel сохраняет исходный кейс', () => {
    expect(parseCalloutMarker('[!TIP] x')?.marker).toEqual({ kind: 'tip', fold: '', rawLabel: 'TIP' });
  });
  it('сворачивание - (свёрнут)', () => {
    expect(parseCalloutMarker('[!info]- Заголовок')?.marker.fold).toBe('-');
  });
  it('сворачивание + (развёрнут, но сворачиваемый)', () => {
    expect(parseCalloutMarker('[!info]+ Заголовок')?.marker.fold).toBe('+');
  });
  it('дефис в типе допустим', () => {
    expect(parseCalloutMarker('[!my-type] t')?.marker.kind).toBe('my-type');
  });
  it('не маркер: обычный текст', () => {
    expect(parseCalloutMarker('обычная цитата')).toBeNull();
  });
  it('не маркер: [!x] в середине строки игнорируется (только начало)', () => {
    expect(parseCalloutMarker('текст [!note] внутри')).toBeNull();
  });
  it('не маркер: пустой тип', () => {
    expect(parseCalloutMarker('[!] t')).toBeNull();
  });
  it('не маркер: тип, начинающийся не с буквы', () => {
    expect(parseCalloutMarker('[!1abc] t')).toBeNull();
  });
});

describe('splitInlineAtNewline', () => {
  it('без переноса — всё заголовок, body=null', () => {
    const res = splitInlineAtNewline([{ type: 'text', value: 'только заголовок' }]);
    expect(res.body).toBeNull();
    expect(res.title).toEqual([{ type: 'text', value: 'только заголовок' }]);
  });
  it('перенос делит на заголовок и тело', () => {
    const res = splitInlineAtNewline([{ type: 'text', value: 'Заголовок\nтело строки' }]);
    expect(res.title).toEqual([{ type: 'text', value: 'Заголовок' }]);
    expect(res.body).toEqual([{ type: 'text', value: 'тело строки' }]);
  });
  it('перенос в начале — пустой заголовок, тело со второй строки', () => {
    const res = splitInlineAtNewline([{ type: 'text', value: '\nтело' }]);
    expect(res.title).toEqual([]);
    expect(res.body).toEqual([{ type: 'text', value: 'тело' }]);
  });
  it('инлайн-разметка в заголовке до переноса сохраняется', () => {
    const strong = { type: 'strong' as const, children: [{ type: 'text' as const, value: 'жирн' }] };
    const res = splitInlineAtNewline([{ type: 'text', value: 'A ' }, strong, { type: 'text', value: '\nbody' }]);
    expect(res.title).toEqual([{ type: 'text', value: 'A ' }, strong]);
    expect(res.body).toEqual([{ type: 'text', value: 'body' }]);
  });
  // Жёсткий перенос (2+ пробела/`\` на конце строки маркера) — remark отдаёт `break`-узлом, не `\n`.
  it('break-узел сразу после маркера: пустой заголовок, тело после', () => {
    const res = splitInlineAtNewline([{ type: 'break' }, { type: 'text', value: 'body' }]);
    expect(res.title).toEqual([]);
    expect(res.body).toEqual([{ type: 'text', value: 'body' }]);
  });
  it('break-узел делит заголовок и тело (заголовок не поглощает тело)', () => {
    const res = splitInlineAtNewline([
      { type: 'text', value: 'Heads up' },
      { type: 'break' },
      { type: 'text', value: 'real body' },
    ]);
    expect(res.title).toEqual([{ type: 'text', value: 'Heads up' }]);
    expect(res.body).toEqual([{ type: 'text', value: 'real body' }]);
  });
});

/** Хелпер: строит минимальный mdast-blockquote с одним абзацем-текстом. */
function bq(text: string): Blockquote {
  return { type: 'blockquote', children: [{ type: 'paragraph', children: [{ type: 'text', value: text }] }] };
}
/** Хелпер: достаёт hName/hProperties узла. */
function hast(node: unknown): { hName?: string; hProperties?: Record<string, unknown> } {
  return (node as { data?: { hName?: string; hProperties?: Record<string, unknown> } }).data ?? {};
}

describe('remarkCallouts (трансформация дерева)', () => {
  it('callout → nexus-callout с kind/fold, первый ребёнок — заголовок', () => {
    const tree: Root = { type: 'root', children: [bq('[!warning]+ Опасно\nтело')] };
    remarkCallouts()(tree);
    const callout = tree.children[0];
    expect(hast(callout).hName).toBe('nexus-callout');
    expect(hast(callout).hProperties).toEqual({ kind: 'warning', fold: '+' });
    const title = (callout as Blockquote).children[0] as Paragraph;
    expect(hast(title).hName).toBe('nexus-callout-title');
    expect(hast(title).hProperties).toEqual({ kind: 'warning', label: 'warning' });
    expect(title.children).toEqual([{ type: 'text', value: 'Опасно' }]);
    // тело — абзац со второй строкой
    const bodyPara = (callout as Blockquote).children[1] as Paragraph;
    expect(bodyPara.children).toEqual([{ type: 'text', value: 'тело' }]);
  });

  it('не-callout blockquote не трогается', () => {
    const tree: Root = { type: 'root', children: [bq('обычная цитата')] };
    remarkCallouts()(tree);
    expect(hast(tree.children[0]).hName).toBeUndefined();
  });

  it('callout без тела: только заголовок-маркер', () => {
    const tree: Root = { type: 'root', children: [bq('[!note] Просто заметка')] };
    remarkCallouts()(tree);
    const callout = tree.children[0] as Blockquote;
    expect(hast(callout).hName).toBe('nexus-callout');
    expect(callout.children).toHaveLength(1); // только заголовок, тела нет
    expect(hast(callout.children[0]).hName).toBe('nexus-callout-title');
  });

  it('fold по умолчанию пустой (не сворачиваемый) → fold:undefined в hProperties', () => {
    const tree: Root = { type: 'root', children: [bq('[!tip] hint')] };
    remarkCallouts()(tree);
    expect(hast(tree.children[0]).hProperties).toEqual({ kind: 'tip', fold: undefined });
  });
});
