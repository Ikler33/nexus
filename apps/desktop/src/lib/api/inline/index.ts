import { Channel, invoke } from '@tauri-apps/api/core';
import * as mockVault from '../../mock/vault';
import { isTauri } from '../bridge';
import type { InlineMode, InlineStreamEvent } from './types';

/**
 * Inline-домен (F-2d): inline-генерация в редакторе (IL-1/2) — стрим результата (`token`… →
 * `done`|`error`) с отменой. Потребители ходят сюда по-прежнему через `tauriApi.inline`
 * (barrel-реэкспорт в `lib/tauri-api.ts`). Вне Tauri — мок-стрим (`lib/mock/vault`).
 */
export const inline = {
  /**
   * Inline-генерация в редакторе (IL-1/2): стрим результата в `onEvent` (`token`… → `done`|`error`).
   * `mode` — `continue`/`rewrite`/`summarize`/`prompt`; `context` — текст до курсора (или вся заметка
   * как контекст для `prompt`); `selection` — выделение (rewrite/summarize); `prompt` — свободный
   * запрос пользователя (⌘/ prompt-box). Возвращает функцию отмены (взводит `inline_cancel`). Вне
   * Tauri — мок.
   *
   * Честное bridge-исключение (см. шапку `../bridge.ts`): стрим-команда с `Channel` (как
   * `chat.streamRag`/`agent.run`) — канал + `onmessage` + функция отмены (отдельная `inline_cancel`) —
   * это не request/response-форма `bridge`, поэтому остаётся прямым `invoke`.
   */
  complete: (
    mode: InlineMode,
    context: string,
    selection: string | undefined,
    onEvent: (event: InlineStreamEvent) => void,
    prompt?: string,
  ): (() => void) => {
    if (!isTauri()) return mockVault.streamInline(mode, onEvent, prompt);
    const channel = new Channel<InlineStreamEvent>();
    channel.onmessage = onEvent;
    invoke<void>('inline_complete', { mode, context, selection, prompt, channel }).catch(
      (e: unknown) => onEvent({ type: 'error', message: String(e) }),
    );
    return () => {
      void invoke<void>('inline_cancel');
    };
  },
};
