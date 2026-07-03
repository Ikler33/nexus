import { activityBar, expect, openVault, test } from './fixtures';
import type { Locator, Page } from '@playwright/test';

/**
 * настройка персистится (спека P0-3 §3.6): тумблер «Чистые ссылки (Live Preview)» —
 * localStorage-преф `nexus.editor.wikilinkLivePreview` (stores/prefs.ts) обязан пережить
 * reload страницы. Дефолт — ВКЛ; выключаем → reload → проверяем → возвращаем ВКЛ.
 */

/** Секция «Редактор» настроек → ряд «Чистые ссылки» (два ряда с одинаковыми Вкл/Выкл — скоуп по тексту). */
async function openCleanLinksRow(page: Page): Promise<Locator> {
  await activityBar(page).getByRole('button', { name: 'Настройки', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Настройки' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Редактор', exact: true }).click();
  const row = dialog.locator('section').filter({ hasText: 'Чистые ссылки (Live Preview)' });
  await expect(row).toBeVisible();
  return row;
}

test('«Чистые ссылки»: Выкл → reload → состояние сохранено → вернуть Вкл', async ({ page }) => {
  let row = await openCleanLinksRow(page);
  // Дефолт — ВКЛ.
  await expect(row.getByRole('button', { name: 'Вкл', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  );

  // Выключаем.
  await row.getByRole('button', { name: 'Выкл', exact: true }).click();
  await expect(row.getByRole('button', { name: 'Выкл', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  );

  // Reload: vault-стор сбрасывается (браузер-мок) → welcome → открыть vault заново.
  await page.reload();
  await openVault(page);
  row = await openCleanLinksRow(page);
  await expect(row.getByRole('button', { name: 'Выкл', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  );

  // Возвращаем обратно ВКЛ (сценарий не оставляет за собой изменённое состояние).
  await row.getByRole('button', { name: 'Вкл', exact: true }).click();
  await expect(row.getByRole('button', { name: 'Вкл', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
});
