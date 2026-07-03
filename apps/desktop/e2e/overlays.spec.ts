import {
  activityBar,
  expect,
  openAiInsight,
  openFileFromTree,
  runPaletteCommand,
  test,
} from './fixtures';

/**
 * overlay-смоук (спека P0-3 §3.2): каждый оверлей/панель открывается из UI, показывает якорь
 * и закрывается. Семантика закрытия кодифицирована КАК ЕСТЬ (baseline-фиксация, не редизайн):
 *
 * - Esc ЗАКРЫВАЕТ панели с focus-trap (hooks/useFocusTrap.ts: Tasks/Inbox/Goals/Memory/Episodes/
 *   Digest/Contradictions/Settings/Cheatsheet) и палитру (свой onKeyDown).
 * - Esc НЕ закрывает Граф/Sync/Plugins by design: у них нет Esc-обработчика, а глобальный Esc
 *   App.tsx выходит только из reading-режима и явно пропускает открытые оверлеи
 *   (reading-esc-precedence). Закрытие — явной кнопкой (aria-label «Закрыть…»).
 */

test('Граф: открывается из ActivityBar, Esc НЕ закрывает (as-is), закрывает кнопка', async ({
  page,
}) => {
  await activityBar(page).getByRole('button', { name: 'Граф', exact: true }).click();
  const closeBtn = page.getByRole('button', { name: 'Закрыть граф' });
  await expect(closeBtn).toBeVisible();
  // Baseline: у графа нет Esc-обработчика закрытия (GraphView гасит Esc только в своём поиске).
  await page.keyboard.press('Escape');
  await expect(closeBtn).toBeVisible();
  await closeBtn.click();
  await expect(closeBtn).toBeHidden();
});

test('Задачи: ActivityBar → диалог с пустым состоянием → Esc закрывает', async ({ page }) => {
  await activityBar(page).getByRole('button', { name: 'Задачи', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Задачи' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/Открытых задач нет/)).toBeVisible(); // мок-волт без `- [ ]`
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Входящие: ActivityBar → диалог с пустым состоянием → Esc закрывает', async ({ page }) => {
  await activityBar(page).getByRole('button', { name: 'Входящие', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Входящие' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/Входящие пусты/)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Синхронизация: ActivityBar → диалог, Esc НЕ закрывает (as-is), закрывает крестик', async ({
  page,
}) => {
  await activityBar(page)
    .getByRole('button', { name: 'Синхронизация (git)', exact: true })
    .click();
  const dialog = page.getByRole('dialog', { name: 'Синхронизация' });
  await expect(dialog).toBeVisible();
  // Baseline: SyncPanel без focus-trap и без keydown — Esc не закрывает.
  await page.keyboard.press('Escape');
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Закрыть', exact: true }).click();
  await expect(dialog).toBeHidden();
});

test('Плагины: палитра → диалог, Esc НЕ закрывает (as-is), закрывает крестик', async ({
  page,
}) => {
  await runPaletteCommand(page, 'Плагины');
  const dialog = page.getByRole('dialog', { name: 'Менеджер плагинов' });
  await expect(dialog).toBeVisible();
  // Baseline: PluginsPanel без focus-trap и без keydown — Esc не закрывает.
  await page.keyboard.press('Escape');
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Закрыть', exact: true }).click();
  await expect(dialog).toBeHidden();
});

test('Цели: меню «AI-инсайты» → диалог с мок-целями → Esc закрывает', async ({ page }) => {
  await openAiInsight(page, 'Цели');
  const dialog = page.getByRole('dialog', { name: 'Цели' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText('Дописать книгу')).toBeVisible(); // мок-цель с прогрессом 65%
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Память ИИ: палитра → диалог → Esc закрывает', async ({ page }) => {
  await runPaletteCommand(page, 'Память ИИ');
  const dialog = page.getByRole('dialog', { name: 'Память ИИ' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/Память пуста/)).toBeVisible(); // мок-фактов нет
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Эпизоды: Настройки → AI / Модели → «Эпизоды…» (Настройки гаснут) → Esc закрывает', async ({
  page,
}) => {
  await activityBar(page).getByRole('button', { name: 'Настройки', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Настройки' });
  await expect(settings).toBeVisible();
  await settings.getByRole('button', { name: 'AI / Модели' }).click();
  await settings.getByRole('button', { name: 'Эпизоды…' }).click();
  const episodes = page.getByRole('dialog', { name: 'Эпизоды' });
  await expect(episodes).toBeVisible();
  // openEpisodes (stores/ui.ts): trap-оверлеи не стекаются — Настройки обязаны погаснуть.
  await expect(settings).toBeHidden();
  await expect(episodes.getByText('Настройка SearXNG на VPS')).toBeVisible(); // мок-эпизод
  await page.keyboard.press('Escape');
  await expect(episodes).toBeHidden();
});

test('Дайджест изменений: меню «AI-инсайты» → диалог с мок-дайджестом → Esc закрывает', async ({
  page,
}) => {
  await openAiInsight(page, 'Дайджест изменений');
  const dialog = page.getByRole('dialog', { name: 'Дайджест изменений' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/Доработана глава/)).toBeVisible(); // мок-дайджест
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Поиск противоречий: меню «AI-инсайты» → диалог с мок-находками → Esc закрывает', async ({
  page,
}) => {
  await openAiInsight(page, 'Поиск противоречий');
  const dialog = page.getByRole('dialog', { name: 'Поиск противоречий' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/одна заметка устарела/)).toBeVisible(); // мок-противоречие
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Настройки: ActivityBar → диалог → Esc закрывает', async ({ page }) => {
  await activityBar(page).getByRole('button', { name: 'Настройки', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Настройки' });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Палитра команд: кнопка поиска титлбара → диалог → Esc закрывает', async ({ page }) => {
  await page.getByRole('button', { name: /Поиск файлов и команд/ }).click();
  const dialog = page.getByRole('dialog', { name: 'Палитра команд' });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Горячие клавиши: палитра → диалог-шпаргалка → Esc закрывает', async ({ page }) => {
  await runPaletteCommand(page, 'Горячие клавиши');
  const dialog = page.getByRole('dialog', { name: 'Горячие клавиши' });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Быстрая запись: палитра → мини-модалка (CAP-2) → Esc закрывает', async ({ page }) => {
  await runPaletteCommand(page, 'Быстрая запись');
  const dialog = page.getByRole('dialog', { name: 'Быстрая запись' });
  await expect(dialog).toBeVisible();
  // Esc обрабатывает сам input (autoFocus) — фокус обязан быть в нём, иначе Esc мёртв.
  await expect(dialog.getByPlaceholder(/Запишите мысль/)).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Новая из шаблона: палитра → пикер с мок-шаблонами (CAP-3) → Esc закрывает', async ({
  page,
}) => {
  await runPaletteCommand(page, 'Новая заметка из шаблона');
  const dialog = page.getByRole('dialog', { name: 'Новая из шаблона' });
  await expect(dialog).toBeVisible();
  // Мок-волт содержит Templates/Meeting.md и Templates/Daily.md.
  await expect(dialog.getByRole('option', { name: /Meeting/ })).toBeVisible();
  await expect(dialog.getByRole('option', { name: /Daily/ })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('История версий: файл открыт → команда палитры → диалог → Esc закрывает', async ({
  page,
}) => {
  // Версии привязаны к активной заметке (SAFE-6) — сначала открываем файл.
  await openFileFromTree(page, /^README/);
  await runPaletteCommand(page, 'История версий');
  const dialog = page.getByRole('dialog', { name: 'История версий' });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('Режим чтения: команда палитры → хром скрыт → Esc-прецедент оверлея → Esc выходит', async ({
  page,
}) => {
  // reading прячет ActivityBar/сайдбар (App.tsx: {!reading && <ActivityBar/>}).
  await runPaletteCommand(page, 'Режим чтения');
  const bar = activityBar(page);
  await expect(bar).toBeHidden();

  // Esc-прецедент (App.tsx reading-Esc-гейт): оверлей поверх reading имеет приоритет — первый
  // Esc закрывает ТОЛЬКО палитру, режим чтения жив (регресс ловился этим смоуком: палитра
  // закрывала себя синхронно, и гейт видел уже-чистый стор → один Esc гасил оба слоя).
  await page.keyboard.press('ControlOrMeta+KeyP');
  const palette = page.getByRole('dialog', { name: 'Палитра команд' });
  await expect(palette).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(palette).toBeHidden();
  await expect(bar).toBeHidden(); // reading всё ещё активен

  // Без оверлеев Esc выходит из чтения.
  await page.keyboard.press('Escape');
  await expect(bar).toBeVisible();
});
