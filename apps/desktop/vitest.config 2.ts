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
  },
});
