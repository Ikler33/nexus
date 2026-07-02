import {
  MAIN_VIEWS,
  activityBar,
  editorContent,
  expect,
  mainViewAnchor,
  openFileFromTree,
  openMainView,
  test,
} from './fixtures';

/**
 * nav-смоук (спека P0-3 §3.1): проводка App ↔ ActivityBar ↔ main-вьюхи.
 * Инвариант SWITCH_MAIN (stores/ui.ts, ST-D1/W-6): main-вьюхи взаимоисключаемы, переход на
 * main-вью гасит плавающие/trap-слои. Юниты сторов это НЕ покрывают — только живой DOM.
 */

test('каждая main-вью открывается из ActivityBar (Home/Сегодня/Новости/Доска/Castor)', async ({
  page,
}) => {
  for (const view of MAIN_VIEWS) {
    await openMainView(page, view); // внутри — ассерт видимого якоря вью
  }
});

test('файлы-редактор: клик по README в дереве открывает CM6 и закрывает Home', async ({
  page,
}) => {
  // Старт — Home (дефолт после открытия vault). Дерево сайдбара видно и из Home.
  await expect(mainViewAnchor(page, 'home')).toBeVisible();
  await openFileFromTree(page, /^README/);
  await expect(editorContent(page)).toBeVisible();
  // Аудит #458: открытие файла обязано УВЕСТИ из main-вью в редактор, не оставив Home поверх.
  await expect(mainViewAnchor(page, 'home')).toBeHidden();
});

test('main↔main: попарные переключения не оставляют прежнюю вью поверх (SWITCH_MAIN)', async ({
  page,
}) => {
  for (const from of MAIN_VIEWS) {
    for (const to of MAIN_VIEWS) {
      if (from === to) continue;
      await openMainView(page, from);
      await openMainView(page, to); // якорь `to` виден
      await expect(mainViewAnchor(page, from)).toBeHidden(); // прежний слой погашен
    }
  }
});

test('переход на main-вью гасит плавающие слои: граф из ActivityBar, trap-оверлей через палитру (ST-D1/W-6)', async ({
  page,
}) => {
  const bar = activityBar(page);

  // Граф — absolute-слой поверх тела: ActivityBar остаётся кликабельным, переход гасит граф.
  await bar.getByRole('button', { name: 'Граф', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Закрыть граф' })).toBeVisible();
  await openMainView(page, 'today');
  await expect(page.getByRole('button', { name: 'Закрыть граф' })).toBeHidden();

  // Trap-оверлей (Задачи): baseline as-is — его бекдроп накрывает ActivityBar/титлбар, мышью
  // туда не попасть. Путь пользователя из-под оверлея — палитра (⌘P: TRAP_OVERLAYS_CLOSED
  // гасит открытый trap-оверлей) → команда «Home» (toggleHome → SWITCH_MAIN гасит today).
  // NB: у «Доски» команды палитры НЕТ (baseline: только кнопка ActivityBar) — поэтому Home.
  await bar.getByRole('button', { name: 'Задачи', exact: true }).click();
  const tasks = page.getByRole('dialog', { name: 'Задачи' });
  await expect(tasks).toBeVisible();
  await page.keyboard.press('ControlOrMeta+KeyP');
  const palette = page.getByRole('dialog', { name: 'Палитра команд' });
  await expect(palette).toBeVisible();
  await expect(tasks).toBeHidden(); // trap-оверлеи взаимоисключаемы (урок P9-ревью #5)
  await palette.getByRole('combobox', { name: 'Палитра команд' }).fill('Home');
  await palette.getByRole('option', { name: /Home/ }).first().click();
  await expect(mainViewAnchor(page, 'home')).toBeVisible();
  await expect(mainViewAnchor(page, 'today')).toBeHidden(); // SWITCH_MAIN: прежняя вью погашена
  await expect(palette).toBeHidden();
  await expect(tasks).toBeHidden();
});
