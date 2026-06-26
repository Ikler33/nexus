import type { Heading, Paragraph, Root, Text } from 'mdast';
import { describe, expect, it } from 'vitest';

import { remarkStripHeadingEmoji } from './remarkStripHeadingEmoji';

/** Дерево: один heading заданного уровня с одним text-ребёнком. */
function headingDoc(depth: 1 | 2 | 3 | 4 | 5 | 6, text: string): Root {
  return {
    type: 'root',
    children: [{ type: 'heading', depth, children: [{ type: 'text', value: text }] }],
  };
}
function headingText(tree: Root): string | undefined {
  const h = tree.children[0] as Heading | undefined;
  const c = h?.children?.[0];
  return c && c.type === 'text' ? c.value : undefined;
}

describe('remarkStripHeadingEmoji', () => {
  it('срезает эмодзи из H2-заголовка (шаблон daily `## 🧠 …`)', () => {
    const tree = headingDoc(2, '🧠 Поток мыслей');
    remarkStripHeadingEmoji()(tree);
    expect(headingText(tree)).toBe('Поток мыслей');
  });

  it('работает на всех уровнях H1–H6', () => {
    for (const depth of [1, 2, 3, 4, 5, 6] as const) {
      const tree = headingDoc(depth, '💡 Идея');
      remarkStripHeadingEmoji()(tree);
      expect(headingText(tree)).toBe('Идея');
    }
  });

  it('НЕ трогает текст абзацев (только заголовки)', () => {
    const tree: Root = {
      type: 'root',
      children: [
        { type: 'heading', depth: 2, children: [{ type: 'text', value: '🧠 Заголовок' }] },
        { type: 'paragraph', children: [{ type: 'text', value: 'Тело с эмодзи 🎉 остаётся' }] },
      ],
    };
    remarkStripHeadingEmoji()(tree);
    expect((tree.children[0] as Heading).children[0]).toMatchObject({ value: 'Заголовок' });
    const para = tree.children[1] as Paragraph;
    expect((para.children[0] as Text).value).toBe('Тело с эмодзи 🎉 остаётся');
  });

  it('заголовок без эмодзи не меняется', () => {
    const tree = headingDoc(2, 'Обычный заголовок');
    remarkStripHeadingEmoji()(tree);
    expect(headingText(tree)).toBe('Обычный заголовок');
  });

  // adversarial FIX 1 (CRITICAL): граничные пробелы МЕЖДУ inline-узлами заголовка НЕ теряются.
  it('заголовок из нескольких inline-узлов: внутренние пробелы целы, границы триммятся', () => {
    // Заголовок `## 📅 До *важно* 🧠 после` → text('📅 До ')+emphasis('важно')+text(' 🧠 после').
    const tree: Root = {
      type: 'root',
      children: [
        {
          type: 'heading',
          depth: 2,
          children: [
            { type: 'text', value: '📅 До ' },
            { type: 'emphasis', children: [{ type: 'text', value: 'важно' }] },
            { type: 'text', value: ' 🧠 после' },
          ],
        },
      ],
    };
    remarkStripHeadingEmoji()(tree);
    const h = tree.children[0] as Heading;
    // ПЕРВЫЙ узел: ведущий «📅 » срезан (leading-trim), но пробел ПЕРЕД emphasis сохранён.
    expect((h.children[0] as Text).value).toBe('До ');
    // ПОСЛЕДНИЙ узел: пробел ПОСЛЕ emphasis сохранён, эмодзи и двойной пробел убраны, хвост триммится.
    expect((h.children[2] as Text).value).toBe(' после');
    // emphasis-текст не тронут
    const emph = h.children[1] as { children: Text[] };
    expect(emph.children[0].value).toBe('важно');
    // Склейки нет: целый текст заголовка читается «До важно после» (пробелы на стыках слов целы).
  });

  it('CRITICAL-регресс: bold-заголовок БЕЗ эмодзи не склеивает слова', () => {
    // `## Раздел **A** и B` → text('Раздел ')+strong('A')+text(' и B'). Без эмодзи трогать нечего,
    // граничные пробелы между узлами обязаны выжить (поузельный trim их бы стёр → «РазделAи B»).
    const tree: Root = {
      type: 'root',
      children: [
        {
          type: 'heading',
          depth: 2,
          children: [
            { type: 'text', value: 'Раздел ' },
            { type: 'strong', children: [{ type: 'text', value: 'A' }] },
            { type: 'text', value: ' и B' },
          ],
        },
      ],
    };
    remarkStripHeadingEmoji()(tree);
    const h = tree.children[0] as Heading;
    expect((h.children[0] as Text).value).toBe('Раздел '); // пробел перед strong цел
    expect((h.children[2] as Text).value).toBe(' и B'); // пробел после strong цел
  });

  // adversarial FIX 2 (MAJOR): text-презентационные символы НЕ режутся.
  it('НЕ срезает text-символы ™ © ® ℹ ↗ ✔ ⚙ (легитимный текст продуктовых/юр-заметок)', () => {
    const tree = headingDoc(2, 'Acme™ © 2026 ® ℹ ↗ ✔ ⚙');
    remarkStripHeadingEmoji()(tree);
    expect(headingText(tree)).toBe('Acme™ © 2026 ® ℹ ↗ ✔ ⚙');
  });

  it('режет символ с emoji-вариантом (VS-16): `⚙️`→∅, но голый `⚙` цел', () => {
    const withVs = headingDoc(2, '⚙️ Настройки');
    remarkStripHeadingEmoji()(withVs);
    expect(headingText(withVs)).toBe('Настройки');
    const without = headingDoc(2, '⚙ Настройки');
    remarkStripHeadingEmoji()(without);
    expect(headingText(without)).toBe('⚙ Настройки'); // без VS-16 — текстовая шестерёнка, не трогаем
  });

  it('убирает emoji-последовательности (флаги, VS-16, ZWJ-склейки)', () => {
    expect(((): string | undefined => {
      const t = headingDoc(2, '🇷🇺 Россия');
      remarkStripHeadingEmoji()(t);
      return headingText(t);
    })()).toBe('Россия');
    expect(((): string | undefined => {
      const t = headingDoc(2, '❤️ Любовь');
      remarkStripHeadingEmoji()(t);
      return headingText(t);
    })()).toBe('Любовь');
    expect(((): string | undefined => {
      const t = headingDoc(2, '👨‍👩‍👧 Семья');
      remarkStripHeadingEmoji()(t);
      return headingText(t);
    })()).toBe('Семья');
  });
});
