import { invoke } from '@tauri-apps/api/core';
import * as mockApp from '../../mock/app';
import { bridge, isTauri } from '../bridge';
import type { BuildInfo } from './types';

/**
 * App-домен (F-2d): нативная мета приложения (версия/git-сборка W-20) и открытие внешних ссылок в
 * СИСТЕМНОМ браузере. Мета-вызовы — через `bridge` (Tauri ↔ мок `lib/mock/app`); потребители ходят
 * сюда по-прежнему через `tauriApi.app`/`tauriApi.external` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const app = {
  /** Версия нативного приложения (Rust-команда `app_version`). Вне Tauri — `dev`. */
  version: (): Promise<string> => bridge<string>('app_version', undefined, () => mockApp.version()),
  /**
   * Git-версия сборки (W-20): `{ version, branch, hash, dirty }`, захвачена `build.rs` на
   * компиляции. Статусбар рисует `ветка @ хеш`, чтобы видеть, ЧТО запущено. Вне Tauri
   * (браузер-превью) — отметка `dev`.
   */
  buildInfo: (): Promise<BuildInfo> =>
    bridge<BuildInfo>('app_build_info', undefined, () => mockApp.buildInfo()),
};

export const external = {
  /**
   * Открывает http(s)-URL в СИСТЕМНОМ браузере (Rust-команда `open_external` через
   * tauri-plugin-opener). В Tauri-вебвью `<a target="_blank">` не открывает браузер (строгий CSP
   * глотает навигацию) — поэтому все внешние ссылки (NF-6 «Оригинал», web-источники чата, ссылки
   * в превью заметок) идут СЮДА. Иные схемы (file:, javascript:) отклоняются и тут, и в Rust.
   * Вне Tauri (браузерное превью) — `window.open`. Открытие — НЕ эгресс приложения (фетчит ОС).
   *
   * Честное исключение (см. шапку `../bridge.ts`): браузерная ветка — прямой `window.open` (навигация
   * ОС, не запрос к мок-бэкенду), поэтому НЕ через `bridge`, а прямой `invoke`/`window.open` с комментом.
   */
  open: (url: string): Promise<void> => {
    if (!/^https?:\/\//i.test(url)) return Promise.reject(new Error('схема не разрешена'));
    if (!isTauri()) {
      window.open(url, '_blank', 'noopener,noreferrer');
      return Promise.resolve();
    }
    return invoke<void>('open_external', { url });
  },
};
