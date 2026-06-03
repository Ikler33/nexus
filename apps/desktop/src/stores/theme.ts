import { create } from 'zustand';

/**
 * Оформление (дизайн-система Hermes): тема (light «old paper» / dark «warm clay»), акцент
 * (data-accent) и плотность (--row-h). Применяется к `<html>` (токены в styles.css per-theme/accent),
 * стартовые значения — из localStorage (без вспышки, side-effect на импорте), смена — с 320ms
 * кросс-фейдом и персистом.
 */
export type Theme = 'light' | 'dark';
export type Accent = 'amber' | 'teal' | 'sage' | 'clay';
export type Density = 'comfortable' | 'compact';

export const ACCENTS: readonly Accent[] = ['amber', 'teal', 'sage', 'clay'];

const THEME_KEY = 'nexus-theme';
const ACCENT_KEY = 'nexus-accent';
const DENSITY_KEY = 'nexus-density';

function readEnum<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    if (v && (allowed as readonly string[]).includes(v)) return v as T;
  } catch {
    /* localStorage недоступен */
  }
  return fallback;
}

function initialTheme(): Theme {
  const saved = readEnum<Theme>(THEME_KEY, ['light', 'dark'], '' as Theme);
  if (saved) return saved;
  return typeof window !== 'undefined' &&
    window.matchMedia?.('(prefers-color-scheme: dark)').matches
    ? 'dark'
    : 'light';
}

function applyTheme(theme: Theme): void {
  if (typeof document !== 'undefined') document.documentElement.dataset.theme = theme;
}
function applyAccent(accent: Accent): void {
  if (typeof document !== 'undefined') document.documentElement.dataset.accent = accent;
}
function applyDensity(density: Density): void {
  if (typeof document !== 'undefined') {
    document.documentElement.style.setProperty('--row-h', density === 'compact' ? '24px' : '28px');
  }
}

function persist(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* ignore */
  }
}

/** Кросс-фейд (.theme-anim на <html>, см. styles.css) на время визуального изменения. */
function withCrossfade(fn: () => void): void {
  if (typeof document === 'undefined') return fn();
  const root = document.documentElement;
  root.classList.add('theme-anim');
  fn();
  window.setTimeout(() => root.classList.remove('theme-anim'), 320);
}

const START_THEME = initialTheme();
const START_ACCENT = readEnum<Accent>(ACCENT_KEY, ACCENTS, 'amber');
const START_DENSITY = readEnum<Density>(DENSITY_KEY, ['comfortable', 'compact'], 'comfortable');
// Применяем до первого рендера (без вспышки).
applyTheme(START_THEME);
applyAccent(START_ACCENT);
applyDensity(START_DENSITY);

interface ThemeState {
  theme: Theme;
  accent: Accent;
  density: Density;
  toggle: () => void;
  setTheme: (theme: Theme) => void;
  setAccent: (accent: Accent) => void;
  setDensity: (density: Density) => void;
}

export const useThemeStore = create<ThemeState>((set) => ({
  theme: START_THEME,
  accent: START_ACCENT,
  density: START_DENSITY,
  toggle: () =>
    set((s) => {
      const next: Theme = s.theme === 'dark' ? 'light' : 'dark';
      persist(THEME_KEY, next);
      withCrossfade(() => applyTheme(next));
      return { theme: next };
    }),
  setTheme: (theme) =>
    set(() => {
      persist(THEME_KEY, theme);
      withCrossfade(() => applyTheme(theme));
      return { theme };
    }),
  setAccent: (accent) =>
    set(() => {
      persist(ACCENT_KEY, accent);
      withCrossfade(() => applyAccent(accent));
      return { accent };
    }),
  setDensity: (density) =>
    set(() => {
      persist(DENSITY_KEY, density);
      applyDensity(density);
      return { density };
    }),
}));
