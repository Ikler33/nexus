import type { BuildInfo } from '../tauri-api';

/**
 * Мок app-домена для браузерного превью / vitest (вне Tauri): нативной сборки нет — версия/git-инфо
 * помечаются `dev` (mock-must-match-backend: реальные команды `app_version`/`app_build_info` дают
 * захваченные `build.rs` значения; в превью честная заглушка `dev`).
 */
export async function version(): Promise<string> {
  return 'dev';
}

export async function buildInfo(): Promise<BuildInfo> {
  return { version: 'dev', branch: 'dev', hash: '', dirty: false };
}
