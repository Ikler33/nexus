/**
 * F-9 — news как ПЕРВЫЙ реально вырезанный модуль (пилот коннектора F-8, REFACTOR-PLAN §5).
 * ЭТАЛОН «как вырезать модуль» (шаблон для F-10-серии): весь вклад news в ядро идёт через `ctx`
 * (main-вью + секция настроек + команда палитры), БЕЗ единой правки ядро-компонентов. Инвариант:
 * ядро (App/ActivityBar/SettingsView/MainViewOutlet/core-views) больше НЕ импортирует
 * `components/news` — вклады отдаёт реестр коннектора. Behavior-preserving: order/icon/titleKey/nav
 * перенесены КАК ЕСТЬ из прежних core-views (вью), SettingsView.CORE_SETTINGS_SECTIONS (секция) и
 * commands-core (команда).
 *
 * Шаблон для F-10: (1) импортировать компоненты фичи, (2) в `activate(ctx)` зарегистрировать вклады
 * через `ctx.views/settings/commands/events`, (3) добавить модуль в `modules/index`. Файл живёт вне
 * `src/components/**`, поэтому импорт компонентов фичи здесь легален (F-1 линт границ стережёт ТОЛЬКО
 * кросс-импорты между зонами `components/<feature>`; модуль — слой проводки, единственное разрешённое
 * место, где `components/news` импортируется).
 *
 * Rust-crate `news` (бэкенд ленты) — ВНЕ скоупа F-9 (сервер-паритет): вырезается только фронт.
 */
import { Newspaper } from 'lucide-react';
import { NewsView } from '../../../components/news/NewsView';
import { NewsSettingsSection } from '../../../components/settings/NewsSettingsSection';
import { useUIStore } from '../../../stores/ui';
import type { NexusModule } from '../types';

/** Модуль «Лента новостей» (NF-*). Пилот вырезания через коннектор F-8. */
export const newsModule: NexusModule = {
  id: 'news',
  activate(ctx) {
    // Main-вью + кнопка ActivityBar. order=30 (между «Сегодня»=20 и «Доска»=40) — как в core-views.
    ctx.views.register({
      id: 'news',
      titleKey: 'commands.view.news',
      icon: Newspaper,
      order: 30,
      component: NewsView,
      activityBar: true,
      // Нав-действие — существующий экшен ui-стора (openNews гасит плавающие/trap-слои, SWITCH_MAIN).
      activate: () => useUIStore.getState().openNews(),
      isActive: (v) => v === 'news',
    });

    // Секция настроек «Новости». order=50 (между «AI»=40 и «Данные»=60) — как в CORE_SETTINGS_SECTIONS.
    ctx.settings.register({
      id: 'news',
      titleKey: 'settings.news.title',
      icon: Newspaper,
      order: 50,
      component: NewsSettingsSection,
    });

    // Команда палитры (прежняя commands-core `view.news`). `ctx.commands` префиксует id модулем →
    // фактический id `news:view.news`, source=`plugin`. Палитра ищет по названию (titleKey) — путь
    // пользователя не меняется. `toggleNews` (не openNews) — тоггл-семантика прежней команды.
    ctx.commands.register({
      id: 'view.news',
      title: 'News feed',
      titleKey: 'commands.view.news',
      run: () => useUIStore.getState().toggleNews(),
    });
  },
};
