import { defineConfig, devices } from '@playwright/test';

/**
 * P0-3 — Playwright-смоук БРАУЗЕРНОЙ сборки (web+mock-слой).
 *
 * ЧЕСТНОСТЬ (W-22): прогон доказывает проводку фронта (App ↔ ActivityBar ↔ оверлеи ↔ редактор ↔
 * мок-бэкенд `lib/mock/*`) в прод-сборке Vite под WebKit — НЕ поведение shipped Tauri-app
 * (Rust-команды, файловая система, окно). Что доказывает / не доказывает — e2e/README.md.
 *
 * Движок: только webkit — ближайший к WKWebView Tauri на macOS; хром не гоняем намеренно.
 * Сервер: `vite preview` (прод-сборка!) на выделенном порту 4173 — НЕ dev-сервер (стабильнее и
 * быстрее, и это та же сборка, что пойдёт в бандл). Порт 1420 занят дев-стендом владельца.
 */
export default defineConfig({
  testDir: '.',
  // Детерминизм > скорость: один воркер, стабильный порядок файлов. Весь смоук — единицы минут.
  fullyParallel: false,
  workers: 1,
  // Без ретраев: ретрай маскирует флейк. Анти-флейк-политика — двойной локальный прогон (гейт среза).
  retries: 0,
  forbidOnly: !!process.env.CI,
  timeout: 30_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI
    ? [['list'], ['html', { outputFolder: 'playwright-report', open: 'never' }]]
    : [['list']],
  outputDir: 'test-results',
  use: {
    baseURL: 'http://localhost:4173',
    locale: 'ru-RU',
    viewport: { width: 1440, height: 900 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'webkit',
      use: { ...devices['Desktop Safari'], viewport: { width: 1440, height: 900 } },
    },
  ],
  webServer: {
    // Прод-сборка + preview на 4173 (strictPort: занят → честный fail, а не тихий сосед-порт).
    command: 'pnpm build && pnpm exec vite preview --port 4173 --strictPort',
    cwd: '..',
    port: 4173,
    reuseExistingServer: !process.env.CI,
    timeout: 240_000,
  },
});
