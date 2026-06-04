import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

// Отдельный конфиг для тестов: vitest подхватит его вместо vite.config.ts.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    css: true,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
    // Coverage-гейт (V1.2, TESTING_STRATEGY §6). Провайдер v8.
    coverage: {
      provider: 'v8',
      // Считаем покрытие по ВСЕМУ src (а не только по импортированному тестами) — иначе новый
      // непокрытый файл «невидим» и храповик его не ловит.
      all: true,
      include: ['src/**'],
      exclude: [
        'src/**/*.{test,spec}.{ts,tsx}',
        'src/test/**',
        'src/main.tsx', // точка входа (бутстрап рендера) — не юнит-тестируется
        'src/**/*.d.ts',
        'src/vite-env.d.ts',
        // SVG/rAF/drag view-слой графа: логика вынесена в graph-sim.ts (юнит-тесты), визуал —
        // проверка человеком (visual-regression — отдельный слой, TESTING_STRATEGY §3/§7).
        'src/components/graph/GraphView.tsx',
      ],
      reporter: ['text-summary', 'json-summary', 'lcov'],
      // Храповик «не ниже»: пороги чуть ниже фактического baseline (запас на шум v8).
      // Замер V1.2: lines/statements 64.0%, functions 61.8%, branches 77.2%.
      // Новый непокрытый код роняет % → CI краснеет (механика «тест на каждую функцию»).
      thresholds: {
        lines: 63,
        statements: 63,
        functions: 60,
        branches: 75,
      },
    },
  },
});
