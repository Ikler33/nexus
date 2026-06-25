import { describe, expect, it } from 'vitest';

import { closeOpenFences } from './closeOpenFences';

describe('closeOpenFences (W-34: толерантность к недописанному код-блоку)', () => {
  it('нечётное число ``` → дорисовывает закрывающий фенс', () => {
    expect(closeOpenFences('```js\nconst x = 1;')).toBe('```js\nconst x = 1;\n```');
  });

  it('чётное число ``` → строка без изменений', () => {
    const s = '```js\nconst x = 1;\n```';
    expect(closeOpenFences(s)).toBe(s);
  });

  it('текст без фенсов → без изменений', () => {
    expect(closeOpenFences('обычный текст без кода')).toBe('обычный текст без кода');
  });

  it('два полных блока (4 ```) → без изменений', () => {
    const s = '```\na\n```\nтекст\n```\nb\n```';
    expect(closeOpenFences(s)).toBe(s);
  });
});
