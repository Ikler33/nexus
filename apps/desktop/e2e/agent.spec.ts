import { expect, openMainView, test, type MainView } from './fixtures';

/**
 * агент-смоук (спека P0-3 §3.5): Castor-прогон против мок-бэкенда (lib/mock/agent.ts, зеркало
 * контракта `agent_*`). Дефолтный мок-прогон эмитит: assistantToken… → toolCall/toolResult →
 * planProposed/subagentStatus → contextUsage → proposal (confirm-гейт ждёт approve) → diff×N →
 * final. Дока «План» показывает РЕАЛЬНЫЕ шаги (W-14), «Граф выполнения» — дерево субагентов
 * (W-24/25), changeset — карточку изменений с exec-силуэтом (`$ git status --short`).
 */

const AGENT: MainView = 'agent';

test('полный прогон: план, дерево субагентов, changeset → «Подтвердить» → свёртка в итог', async ({
  page,
}) => {
  await openMainView(page, AGENT);
  await page.getByPlaceholder('Поручите задачу агенту…').fill('Разбери мои входящие заметки');
  await page.getByRole('button', { name: 'Запустить', exact: true }).click();

  // Стрим ассистента пошёл.
  await expect(page.getByText(/Принял задачу/).first()).toBeVisible();

  // Карточка плана (док «План» открыт по умолчанию): реальные tool-шаги прогона (W-14).
  const planDock = page.locator('aside').filter({ hasText: 'План' });
  await expect(planDock.getByText('fs.read', { exact: true })).toBeVisible();
  await expect(planDock.getByText('note.create', { exact: true })).toBeVisible();

  // Changeset (confirm-гейт): карточка «Изменения», файлы + exec-силуэт, статус «Жду решения».
  // exact: true — тот же путь звучит и в ленте шагов («Создаёт заметку: …»), нужна строка карточки.
  await expect(page.getByText('Изменения', { exact: true })).toBeVisible();
  await expect(page.getByText('RMS-B2B/Идея — кэш контекста.md', { exact: true })).toBeVisible();
  await expect(page.getByText(/\$ git status --short/)).toBeVisible(); // exec-строка без диффа
  await expect(page.locator('[data-status="awaiting"]')).toBeVisible();

  // «Подтвердить» одобряет всё не отклонённое явно → мок применяет файлы → final.
  await page.getByRole('button', { name: 'Подтвердить', exact: true }).click();
  // EDFIX-регрессия: карточка сворачивается в строку-итог («применено: N»), список файлов гаснет.
  // (Проверяем ДО открытия граф-дока: его узлы показывают те же пути в деталях.)
  await expect(page.getByText(/применено: \d+/)).toBeVisible();
  await expect(page.getByText('RMS-B2B/Идея — кэш контекста.md', { exact: true })).toBeHidden();
  await expect(page.locator('[data-status="done"]')).toBeVisible();

  // Дерево субагентов — док «Граф выполнения» (W-24): узел делегирования с целью и итогом.
  // NB: узел живёт в svg>foreignObject — getByText там падает внутренней ошибкой webkit-движка
  // playwright («selector.includes»), поэтому матчим по title-атрибуту подписи узла.
  await page.getByRole('button', { name: 'Граф выполнения', exact: true }).click();
  await expect(page.locator('[title*="Сводка по проекту RMS-B2B"]').first()).toBeVisible();
});

// TODO(P0-2): снять skip после мержа test/p0-2-mock-parity — пара `execProposal`→`execResult`
// (SANDBOX-6c: редакция-безопасный exec-силуэт + exit-код) эмитится моком только там, по узкому
// триггеру `\bexec\b` в тексте задачи («execute…» НЕ триггерит). В текущем main мок шлёт только
// exec-строку внутри proposal (покрыто тестом полного прогона выше).
test.skip('задача с `exec` → exec-пара песочницы (execProposal → execResult) в ленте хода', async ({
  page,
}) => {
  await openMainView(page, AGENT);
  await page.getByPlaceholder('Поручите задачу агенту…').fill('exec: проверь статус репозитория');
  await page.getByRole('button', { name: 'Запустить', exact: true }).click();

  // Силуэт exec-вызова: имя инструмента + счётчики, БЕЗ сырых argv/env (приватность §5.6);
  // затем результат с exit-кодом. Точные якоря уточнить при ребейзе на P0-2.
  await expect(page.getByText(/\$ git status --short/).first()).toBeVisible();
  await expect(page.locator('[data-status="done"]')).toBeVisible();
});

// TODO(P0-2): снять skip после мержа test/p0-2-mock-parity — событие `report` (RES-5) мок эмитит
// только там, по триггеру /report|отч[её]т/ в тексте задачи. Рендер дока «Отчёт» (ReportPane)
// уже есть в main — не хватает именно мок-события.
test.skip('задача с «report» → карточка отчёта в доке «Отчёт»', async ({ page }) => {
  await openMainView(page, AGENT);
  await page.getByPlaceholder('Поручите задачу агенту…').fill('Составь отчёт по входящим');
  await page.getByRole('button', { name: 'Запустить', exact: true }).click();

  await page.getByRole('button', { name: 'Подтвердить', exact: true }).click();
  await expect(page.locator('[data-status="done"]')).toBeVisible();
  // Док «Отчёт»: карточка отчёта прогона (не пустой стейт «Отчёт появится…»).
  await page.getByRole('button', { name: 'Отчёт', exact: true }).click();
  const reportDock = page.locator('aside').filter({ hasText: 'Отчёт' });
  await expect(reportDock.getByText(/Отчёт появится/)).toBeHidden();
});
