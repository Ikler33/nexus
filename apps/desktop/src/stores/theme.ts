import { create } from 'zustand';

/**
 * Тема оформления (дизайн-система Hermes): светлая «old paper» / тёмная «warm clay».
 * Применяется через `data-theme` на `<html>` (токены в styles.css per-theme).
 * Источник при старте: localStorage('nexus-theme'), иначе системная (prefers-color-scheme).
 * Смена — с 320ms кросс-фейдом (класс `.theme-anim`, см. styles.css), персист в localStorage.
 */
export type Theme = 'light' | 'dark';

const STORAGE_KEY = 'nexus-theme';

function initialTheme(): Theme {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === 'light' || saved === 'dark') return saved;
  } catch {
    /* localStorage недоступен (приватный режим/тест) — падаём на системную */
  }
  return typeof window !== 'undefined' &&
    window.matchMedia?.('(prefers-color-scheme: dark)').matches
    ? 'dark'
    : 'light';
}

function applyTheme(theme: Theme): void {
  if (typeof document !== 'undefined') document.documentElement.dataset.theme = theme;
}

function persistAndApply(theme: Theme): void {
  try {
    localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    /* ignore */
  }
  if (typeof document !== 'undefined') {
    const root = document.documentElement;
    root.classList.add('theme-anim');
    applyTheme(theme);
    window.setTimeout(() => root.classList.remove('theme-anim'), 320);
  }
}

// Применяем тему до первого рендера (без вспышки): сторонний эффект на импорте модуля.
const START = initialTheme();
applyTheme(START);

interface ThemeState {
  theme: Theme;
  toggle: () => void;
  setTheme: (theme: Theme) => void;
}

export const useThemeStore = create<ThemeState>((set) => ({
  theme: START,
  toggle: () =>
    set((s) => {
      const next: Theme = s.theme === 'dark' ? 'light' : 'dark';
      persistAndApply(next);
      return { theme: next };
    }),
  setTheme: (theme) =>
    set(() => {
      persistAndApply(theme);
      return { theme };
    }),
}));
