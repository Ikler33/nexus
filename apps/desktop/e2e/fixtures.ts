import { test as base, expect, type Locator, type Page } from '@playwright/test';

/**
 * Общие фикстуры P0-3-смоука (браузерная сборка + мок-слой `lib/mock/*`).
 *
 * Бутстрап-рецепт (проверен в scratchpad/pwtest/smoke2.mjs): онбординг помечен пройденным ДО
 * загрузки (localStorage `nexus.onboarded.v1=1`, DP-7) → welcome сразу показывает «Открыть vault»
 * → клик открывает мок-волт (вне Tauri `openVaultFlow` идёт в lib/mock/vault.ts) → app-shell.
 *
 * console-гейт (спека P0-3 §3): КАЖДЫЙ тест валится, если страница выдала console.error или
 * pageerror. Смоук — единственный межкомпонентный оракул фронта; молчаливые ошибки в консоли —
 * ровно тот класс регрессий, который юниты сторов не ловят.
 */

/**
 * Белый список ДОПУСТИМЫХ console.error-паттернов. Заведён пустым НАМЕРЕННО (спека §3):
 * расширять только с комментарием-обоснованием, почему конкретная ошибка допустима и
 * почему её нельзя починить в src.
 */
const CONSOLE_ERROR_WHITELIST: RegExp[] = [];

export const test = base.extend({
  // Второй параметр фикстуры playwright канонически зовётся `use`, но так его ловит
  // react-hooks/rules-of-hooks (это не React-код) — потому `provide`.
  page: async ({ page }, provide) => {
    const violations: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() !== 'error') return;
      const text = msg.text();
      if (CONSOLE_ERROR_WHITELIST.some((re) => re.test(text))) return;
      violations.push(`console.error: ${text}`);
    });
    page.on('pageerror', (err) => violations.push(`pageerror: ${err.message}`));

    await page.addInitScript(() => localStorage.setItem('nexus.onboarded.v1', '1'));
    await page.goto('/');
    await openVault(page);

    await provide(page);

    expect(
      violations,
      'console-гейт: страница не должна эмитить console.error/pageerror (whitelist пуст — см. fixtures.ts)',
    ).toEqual([]);
  },
});

export { expect };

/**
 * Кликает «Открыть vault» на welcome-экране и ждёт app-shell. После `page.reload()` vault-стор
 * сбрасывается (браузер-мок не персистит vault) — вызывать снова.
 */
export async function openVault(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Открыть vault' }).click();
  await expect(activityBar(page)).toBeVisible();
}

/**
 * ActivityBar — ПЕРВЫЙ nav с aria-label «Навигация» (side-нав HomeView носит тот же label;
 * ActivityBar в DOM раньше — appShell рендерит его первым ребёнком).
 */
export function activityBar(page: Page): Locator {
  return page.getByRole('navigation', { name: 'Навигация' }).first();
}

/** Полноэкранные main-вью (MAIN_VIEWS_CLOSED из stores/ui.ts) и их кнопки ActivityBar. */
export type MainView = 'home' | 'today' | 'news' | 'board' | 'agent';

export const MAIN_VIEWS: readonly MainView[] = ['home', 'today', 'news', 'board', 'agent'];

/** aria-label кнопки ActivityBar (i18n ru: commands.view.*). */
const MAIN_VIEW_BUTTON: Record<MainView, string> = {
  home: 'Home',
  today: 'Сегодня',
  news: 'Новости',
  board: 'Доска',
  agent: 'Castor',
};

/** Стабильный видимый якорь каждой main-вью (по факту разметки компонентов). */
export function mainViewAnchor(page: Page, view: MainView): Locator {
  switch (view) {
    case 'home':
      // HomeView: <main aria-label="Home"> (home.title).
      return page.getByRole('main', { name: 'Home' });
    case 'today':
      // TodayView: <h1>Сегодня</h1> (today.title).
      return page.getByRole('heading', { name: 'Сегодня', level: 1 });
    case 'news':
      // NewsView: <main aria-label="Новости"> (news.title).
      return page.getByRole('main', { name: 'Новости' });
    case 'board':
      // BoardView: <h1>Доска</h1> (board.title).
      return page.getByRole('heading', { name: 'Доска', level: 1 });
    case 'agent':
      // AgentView: композер агента (agent.composer.placeholder) — уникален для вью.
      return page.getByPlaceholder('Поручите задачу агенту…');
  }
}

/** Открывает main-вью кликом по её кнопке ActivityBar и ждёт её якорь. */
export async function openMainView(page: Page, view: MainView): Promise<void> {
  await activityBar(page)
    .getByRole('button', { name: MAIN_VIEW_BUTTON[view], exact: true })
    .click();
  await expect(mainViewAnchor(page, view)).toBeVisible();
}

/** Редактор (CM6): contenteditable-поверхность внутри [data-testid="editor"]. */
export function editorContent(page: Page): Locator {
  return page.locator('[data-testid="editor"] .cm-content');
}

/** Открывает файл кликом в дереве сайдбара (role=treeitem, имя без .md) и ждёт CM6. */
export async function openFileFromTree(page: Page, name: RegExp): Promise<void> {
  await page.getByRole('tree', { name: 'Файлы vault' }).getByRole('treeitem', { name }).click();
  await expect(editorContent(page)).toBeVisible();
}

/** Открывает палитру (кнопка поиска в титлбаре) и запускает команду по имени. */
export async function runPaletteCommand(page: Page, command: string): Promise<void> {
  await page.getByRole('button', { name: /Поиск файлов и команд/ }).click();
  const dialog = page.getByRole('dialog', { name: 'Палитра команд' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('combobox', { name: 'Палитра команд' }).fill(command);
  // Клик по пункту флейкал на CI: выдача пере-рендеривается под кликом («not stable», таймаут).
  // Клавиатурный коммит устойчив: доводим АКТИВНЫЙ пункт (aria-selected) до целевого стрелкой
  // (ограниченно) и жмём Enter — финальный web-first ассерт гарантирует детерминизм.
  const target = dialog.getByRole('option', { name: new RegExp(escapeRegExp(command)) }).first();
  await expect(target).toBeVisible();
  for (let i = 0; i < 12; i++) {
    if ((await target.getAttribute('aria-selected')) === 'true') break;
    await page.keyboard.press('ArrowDown');
  }
  await expect(target).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Enter');
}

/** Открывает пункт меню «AI-инсайты» титлбара (Дайджест/Цели/Противоречия). */
export async function openAiInsight(page: Page, item: string): Promise<void> {
  await page.getByRole('button', { name: 'AI-инсайты' }).click();
  await page.getByRole('menuitem', { name: item }).click();
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
