import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// @tauri-apps/cli задаёт TAURI_DEV_HOST для разработки на физическом устройстве.
const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/ — конфиг настроен под Tauri 2 (фикс. порт, без очистки экрана).
export default defineConfig({
  plugins: [react()],
  // Tauri пишет собственные логи в этот же терминал — не очищаем экран Vite.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: 'ws', host, port: 1421 }
      : undefined,
    watch: {
      // src-tauri собирается Rust-тулчейном — Vite его не должен слушать.
      ignored: ['**/src-tauri/**'],
    },
  },
  // (Граф переписан на чистый SVG — тяжёлых лениво-подгружаемых граф-зависимостей больше нет, поэтому
  // прежний `optimizeDeps.include` для sigma/graphology удалён вместе с самими зависимостями.)
  // Переменные окружения Tauri доступны во фронте.
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    // Цель webview по платформам (Tauri рекомендация).
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
