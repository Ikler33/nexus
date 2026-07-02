import { editorContent, expect, openFileFromTree, test } from './fixtures';

/**
 * editor/preview-смоук (спека P0-3 §3.3): source-режим (CM6) → Live Preview вики-ссылки
 * (алиас схлопывается, LP/EDFIX-4) → preview: буквица `p[data-dropcap]` + race-регрессия
 * EDFIX-4 (повторное открытие preview сохраняет буквицу) + чистые ссылки без видимых `[[`.
 */

test('README: source → LP-алиас → preview (буквица, чистые ссылки) → повторный preview', async ({
  page,
}) => {
  await openFileFromTree(page, /^README/);
  const cm = editorContent(page);

  // Дефолтный режим — source: CM6 виден, существующие ссылки README уже схлопнуты LP
  // (дефолт префа «Чистые ссылки (Live Preview)» — ВКЛ).
  await expect(cm).toBeVisible();

  // Набор `[[README|Алиас]]` в конец документа: курсор в последнюю строку → End → новая строка.
  await cm.locator('.cm-line').last().click();
  await page.keyboard.press('End');
  await page.keyboard.type('\n[[README|Алиас]]');

  // LP (EDFIX-4): после закрытия `]]` курсор вне диапазона → ссылка схлопнута в «Алиас»
  // (Decoration.replace прячет `[[README|` и `]]`; mark `.cm-wikilink` держит видимый лейбл).
  await expect(cm.locator('.cm-wikilink').filter({ hasText: 'Алиас' })).toBeVisible();
  await expect(cm).not.toContainText('[[README|');

  // Тоггл в preview (плавающая пилюля; та же команда — ⌘E).
  await page.getByRole('button', { name: 'Просмотр', exact: true }).click();
  const preview = page.locator('[data-clean-links]'); // контейнер preview: чистые ссылки ВКЛ
  await expect(preview).toBeVisible();

  // Буквица (EDFIX-4): атрибут стоит на первом «обычном» абзаце.
  await expect(preview.locator('p[data-dropcap]').first()).toBeVisible();

  // Ссылки в preview — БЕЗ видимых `[[`: алиас рендерится как <a data-note="README">Алиас</a>,
  // скобки не входят в текст (при data-clean-links и CSS-скобки ::before/::after погашены).
  await expect(preview.locator('a[data-note="README"]').filter({ hasText: 'Алиас' })).toBeVisible();
  await expect(preview).not.toContainText('[[');

  // Обратно в source и снова в preview — race-регрессия EDFIX-4: буквица обязана пережить
  // повторный маунт MarkdownPreview.
  await page.getByRole('button', { name: 'Исходник', exact: true }).click();
  await expect(cm).toBeVisible();
  await page.getByRole('button', { name: 'Просмотр', exact: true }).click();
  await expect(page.locator('[data-clean-links] p[data-dropcap]').first()).toBeVisible();
});
