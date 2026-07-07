import { beforeEach, describe, expect, it, vi } from 'vitest';

import { openVaultFlow } from './commands-core';
import { useToastStore } from '../stores/toast';
import { useVaultStore } from '../stores/vault';

/** Сообщения текущих тостов (для ассертов). */
const toastMessages = () => useToastStore.getState().toasts.map((t) => t.message);

// Fix BF-1 №3b: ошибка открытия vault (частая на macOS — TCC-запрет, `os error 13`) раньше всплывала
// как js-unhandled-rejection и welcome-экран молчал. openVaultFlow обязан ловить её и показывать
// внятный тост существующим механизмом.
describe('openVaultFlow — ошибки открытия vault (Fix BF-1 №3b)', () => {
  beforeEach(() => {
    useToastStore.setState({ toasts: [] });
  });

  it('TCC/PermissionDenied (os error 13) → подсказка про настройки доступа macOS', async () => {
    useVaultStore.setState({
      openVault: vi.fn().mockRejectedValue('io: Permission denied (os error 13)'),
    });
    await expect(openVaultFlow()).resolves.toBeUndefined(); // НЕ unhandled rejection
    const msgs = toastMessages();
    expect(msgs.some((m) => m.includes('Нет доступа к папке'))).toBe(true);
    expect(msgs.some((m) => m.includes('Конфиденциальность и безопасность'))).toBe(true);
  });

  it('произвольная ошибка → generic-тост с текстом (НЕ маскируется под TCC)', async () => {
    useVaultStore.setState({
      openVault: vi.fn().mockRejectedValue(new Error('диск отвалился')),
    });
    await expect(openVaultFlow()).resolves.toBeUndefined();
    const msgs = toastMessages();
    expect(
      msgs.some((m) => m.includes('Не удалось открыть папку') && m.includes('диск отвалился')),
    ).toBe(true);
    // generic-ошибка НЕ показывает TCC-подсказку (не путаем пользователя).
    expect(msgs.some((m) => m.includes('Нет доступа к папке'))).toBe(false);
  });
});
