import { describe, expect, it } from 'vitest';

import { DARK_THEMES, isDarkTheme, THEMES, type Theme } from './theme';

/** Светлые темы (color-scheme: light в tokens.css) — дополнение к DARK_THEMES. */
const LIGHT_THEMES: Theme[] = ['light', 'paper', 'sepia', 'marble'];

describe('DARK_THEMES (канон светлота темы)', () => {
  it('каждая тема классифицирована ровно один раз (drift-guard: новая тема обязана попасть в dark или light)', () => {
    for (const t of THEMES) {
      const dark = DARK_THEMES.has(t);
      const light = LIGHT_THEMES.includes(t);
      expect(dark !== light, `тема '${t}' должна быть либо dark, либо light (не обе/ни одной)`).toBe(
        true,
      );
    }
    // покрытие полное: dark + light = все темы
    expect(DARK_THEMES.size + LIGHT_THEMES.length).toBe(THEMES.length);
  });

  it('isDarkTheme зеркалит DARK_THEMES', () => {
    expect(isDarkTheme('dark')).toBe(true);
    expect(isDarkTheme('bronze')).toBe(true); // антик-тёмная (раньше mermaid/граф её не считали тёмной)
    expect(isDarkTheme('tokyo')).toBe(true);
    expect(isDarkTheme('light')).toBe(false);
    expect(isDarkTheme('marble')).toBe(false); // антик-светлая
    expect(isDarkTheme('sepia')).toBe(false);
  });

  it('все DARK_THEMES — валидные темы из THEMES', () => {
    for (const t of DARK_THEMES) expect(THEMES).toContain(t);
  });
});
