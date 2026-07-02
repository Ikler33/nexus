import { activityBar, expect, test } from './fixtures';

/**
 * чат-смоук (спека P0-3 §3.4): открыть чат → вопрос → мок-стрим (lib/mock/vault.ts streamChat:
 * sources → reasoningSummary → токены → done). Вопрос подобран под мок-ретрив (searchContent
 * ищет по CONTENT мок-волта; «план проекта Alpha» лежит в Projects/Roadmap.md).
 */

test('вопрос → sources-чипы → стрим токенов → done', async ({ page }) => {
  // Кнопка титлбара «AI-панель»: с Home обязана вывести в workspace и показать панель
  // (DP-12 + W-6 «мёртвая кнопка»), а не только взвести флаг.
  await page.getByRole('button', { name: 'AI-панель' }).click();
  const input = page.getByPlaceholder('Спросите о заметках…');
  await expect(input).toBeVisible();

  await input.fill('Что за план проекта Alpha?');
  await page.getByRole('button', { name: 'Отправить', exact: true }).click();

  // 1. Источники прилетают ДО токенов (порядок мок-стрима зеркалит бэкенд).
  await expect(page.getByText(/Источники · \d+/)).toBeVisible();
  // 2. Токены текут → финальный текст мок-ответа на месте.
  await expect(page.getByText(/На основе заметок/)).toBeVisible();
  // 3. done: стрим завершён — кнопка «Стоп» ушла, вернулась «Отправить», у сообщения есть действия.
  await expect(page.getByRole('button', { name: 'Стоп', exact: true })).toBeHidden();
  await expect(page.getByRole('button', { name: 'Отправить', exact: true })).toBeVisible();
  // .last(): у восстановленной из мок-истории пары сообщений уже есть свой кебаб действий.
  await expect(page.getByRole('button', { name: 'Действия с сообщением' }).last()).toBeVisible();
  // App жив (console-гейт фикстуры дополнительно проверит чистоту консоли).
  await expect(activityBar(page)).toBeVisible();
});

// TODO(P0-2): снять skip после мержа test/p0-2-mock-parity — узкий демо-маркер «демо-ошибка»/
// «demo-error» в streamChat (терминальный `error`) появляется именно там; в текущем main
// мок-чат ошибку не эмитит вообще.
test.skip('вопрос с «демо-ошибка» → error-состояние без падения app', async ({ page }) => {
  await page.getByRole('button', { name: 'AI-панель' }).click();
  const input = page.getByPlaceholder('Спросите о заметках…');
  await input.fill('демо-ошибка');
  await page.getByRole('button', { name: 'Отправить', exact: true }).click();

  // Терминальный error рендерится в теле сообщения (chat.error = «Ошибка: …»), app живёт дальше.
  await expect(page.getByText(/^Ошибка:/)).toBeVisible();
  await expect(activityBar(page)).toBeVisible();
  // Поле ввода снова доступно — можно задать следующий вопрос.
  await expect(input).toBeEnabled();
});
