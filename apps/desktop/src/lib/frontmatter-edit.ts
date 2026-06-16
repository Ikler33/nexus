// Безопасная запись ОДНОГО frontmatter-ключа (общий путь для DnD-доски BOARD-5 и Properties-панели
// PROP-3). Инкапсулирует урок BOARD-5 R1 (потеря правок тела) + анти-эхо SAFE-3 в одном тестируемом месте.

import { tauriApi } from './tauri-api';
import { useWorkspaceStore } from '../stores/workspace';

/** Ошибка флаша: открытый буфер был грязным, но сохранить на диск не удалось — frontmatter НЕ трогаем. */
export class FlushFailedError extends Error {
  constructor() {
    super('flush-failed');
    this.name = 'FlushFailedError';
  }
}

/**
 * Флашит грязный открытый буфер `path` на диск; не удалось снять dirty → `FlushFailedError`. Нужно перед
 * ЛЮБЫМ нашим чтением/записью диска по этому пути — иначе прочитаем/затрём несохранённые правки тела/свойств
 * (урок BOARD-5 R1: запись читала старый диск; AI-1: guard «уже задача» читал старый status и откатывал бы
 * только что набранное значение колонки). No-op для чистого/закрытого буфера.
 */
export async function flushBufferIfDirty(path: string): Promise<void> {
  const ws = useWorkspaceStore.getState();
  if (ws.buffers[path]?.dirty) {
    await ws.saveBuffer(path, true);
    if (useWorkspaceStore.getState().buffers[path]?.dirty) {
      throw new FlushFailedError();
    }
  }
}

/**
 * Пишет один плоский frontmatter-ключ заметки через `set_frontmatter_field`:
 * 1) если заметка открыта и ГРЯЗНАЯ — сперва флашит тело на диск; не удалось → `FlushFailedError`
 *    (иначе `set_frontmatter_field` прочитал бы старый диск без правок тела, а sync затёр бы их — R1);
 * 2) пишет ключ → получает новый контент+хеш;
 * 3) синхронизирует открытый буфер (doc/baseHash) ДО watcher-события (анти-эхо SAFE-3).
 * Пробрасывает ошибку команды (битый frontmatter / непредставимое значение) — вызывающий откатывает UI.
 */
export async function writeFrontmatterField(
  path: string,
  key: string,
  value: string,
): Promise<void> {
  await flushBufferIfDirty(path);
  const res = await tauriApi.vault.setFrontmatterField(path, key, value);
  useWorkspaceStore.getState().syncBufferAfterWrite(path, res.content, res.hash);
}
