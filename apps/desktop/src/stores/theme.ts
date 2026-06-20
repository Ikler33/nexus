import { create } from 'zustand';

/**
 * Оформление (дизайн-система Qasr): тема (data-theme — 15 тем) , акцент (data-accent)
 * и плотность (--row-h). Применяется к `<html>` (токены в styles.css per-theme/accent),
 * стартовые значения — из localStorage (без вспышки, side-effect на импорте), смена —
 * с 320ms кросс-фейдом и персистом. Точные строки = data-theme-значения в styles.css.
 */
export type Theme =
  | 'light'
  | 'dark'
  | 'midnight'
  | 'platinum'
  | 'paper'
  | 'mocha'
  | 'nord'
  | 'tokyo'
  | 'rose'
  | 'sepia'
  | 'contrast'
  | 'bronze'
  | 'marble';
export type Accent = 'amber' | 'teal' | 'sage' | 'clay';
export type Density = 'comfortable' | 'compact' | 'auto';
export type Chrome = 'standard' | 'minimal';
export type EditorFont = 'sans' | 'serif' | 'mono';

export const THEMES: readonly Theme[] = [
  'light',
  'dark',
  'midnight',
  'platinum',
  'paper',
  'mocha',
  'nord',
  'tokyo',
  'rose',
  'sepia',
  'contrast',
  'bronze',
  'marble',
];
export const ACCENTS: readonly Accent[] = ['amber', 'teal', 'sage', 'clay'];

const THEME_KEY = 'nexus-theme';
const ACCENT_KEY = 'nexus-accent';
const DENSITY_KEY = 'nexus-density';
const CHROME_KEY = 'nexus-chrome';
const EDITOR_FONT_KEY = 'nexus-editor-font';
/** Брейкпоинт авто-плотности (макет app.jsx): уже — compact. */
const AUTO_COMPACT_BELOW = 1180;

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
  const saved = readEnum<Theme>(THEME_KEY, THEMES, '' as Theme);
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
  if (typeof document === 'undefined') return;
  const compact =
    density === 'compact' ||
    (density === 'auto' && typeof window !== 'undefined' && window.innerWidth < AUTO_COMPACT_BELOW);
  document.documentElement.style.setProperty('--row-h', compact ? '24px' : '28px');
  document.documentElement.style.setProperty('--density', compact ? '0.82' : '1');
}

/** `--chrome` гейтит видимость рамок (см. styles.css --color-border): minimal = 0. */
function applyChrome(chrome: Chrome): void {
  if (typeof document !== 'undefined') {
    document.documentElement.style.setProperty('--chrome', chrome === 'minimal' ? '0' : '1');
  }
}

function applyEditorFont(font: EditorFont): void {
  if (typeof document === 'undefined') return;
  const value =
    font === 'serif' ? 'var(--font-serif)' : font === 'mono' ? 'var(--font-mono)' : 'var(--font-ui)';
  document.documentElement.style.setProperty('--editor-font', value);
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
const START_DENSITY = readEnum<Density>(
  DENSITY_KEY,
  ['comfortable', 'compact', 'auto'],
  'comfortable',
);
const START_CHROME = readEnum<Chrome>(CHROME_KEY, ['standard', 'minimal'], 'standard');
const START_EDITOR_FONT = readEnum<EditorFont>(EDITOR_FONT_KEY, ['sans', 'serif', 'mono'], 'sans');
// Применяем до первого рендера (без вспышки).
applyTheme(START_THEME);
applyAccent(START_ACCENT);
applyDensity(START_DENSITY);
applyChrome(START_CHROME);
applyEditorFont(START_EDITOR_FONT);
// Авто-плотность реагирует на резайз окна (брейкпоинт 1180, как в макете).
if (typeof window !== 'undefined') {
  window.addEventListener('resize', () => {
    const current = useThemeStore.getState().density;
    if (current === 'auto') applyDensity('auto');
  });
}

interface ThemeState {
  theme: Theme;
  accent: Accent;
  density: Density;
  chrome: Chrome;
  editorFont: EditorFont;
  toggle: () => void;
  setTheme: (theme: Theme) => void;
  setAccent: (accent: Accent) => void;
  setDensity: (density: Density) => void;
  setChrome: (chrome: Chrome) => void;
  setEditorFont: (font: EditorFont) => void;
}

export const useThemeStore = create<ThemeState>((set) => ({
  theme: START_THEME,
  accent: START_ACCENT,
  density: START_DENSITY,
  chrome: START_CHROME,
  editorFont: START_EDITOR_FONT,
  // Цикл по всем 4 темам (DP-4, как в макете: sun → moon → sparkles → drive).
  toggle: () =>
    set((s) => {
      const next = THEMES[(THEMES.indexOf(s.theme) + 1) % THEMES.length];
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
  setChrome: (chrome) =>
    set(() => {
      persist(CHROME_KEY, chrome);
      applyChrome(chrome);
      return { chrome };
    }),
  setEditorFont: (editorFont) =>
    set(() => {
      persist(EDITOR_FONT_KEY, editorFont);
      applyEditorFont(editorFont);
      return { editorFont };
    }),
}));
