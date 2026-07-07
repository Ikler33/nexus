/**
 * F-10c — «Доска» (kanban/list board) как вырезанный модуль через views-реестр (F-9 news — эталон
 * вью-модуля). Board — полноэкранная main-вью (mainView='board'), НЕ оверлей: вклад идёт через
 * `ctx.views` (кнопка ActivityBar + MainViewOutlet), ядро (core-views) больше НЕ импортирует
 * `components/board`.
 *
 * ОТЛИЧИЕ от news-эталона: у board НЕТ секции настроек и НЕТ команды палитры (ядро никогда не
 * объявляло `view.board` — вход только через кнопку ActivityBar). Поэтому COMMAND_ID_ALIASES для
 * board НЕ нужен (переименовывать нечего). AI-команда `board.promote` (commands-core) ОСТАЁТСЯ ядром
 * — она зовёт `openBoard()` ui-стора и импортит `lib/board-promote` (не `components/board`), границу
 * не нарушает. Стейт main-вью `mainView` + экшены openBoard/toggleBoard/closeBoard остаются ядром
 * (ui-стор), как для news — модуль лишь даёт КОМПОНЕНТ + нав-действие.
 *
 * Behavior-preserving: order=40/icon=LayoutGrid/titleKey/activate перенесены КАК ЕСТЬ из прежней
 * записи core-views (между «Новости»=30 и «Агент»=50).
 */
import { LayoutGrid } from 'lucide-react';
import { BoardView } from '../../../components/board/BoardView';
import { useUIStore } from '../../../stores/ui';
import type { NexusModule } from '../types';

/** Модуль «Доска» (kanban/list). Вырезан из core-views через `ctx.views` (F-10c). */
export const boardModule: NexusModule = {
  id: 'board',
  activate(ctx) {
    // Main-вью + кнопка ActivityBar. order=40 (между «Новости»=30 и «Агент»=50) — как в core-views.
    ctx.views.register({
      id: 'board',
      titleKey: 'commands.view.board',
      icon: LayoutGrid,
      order: 40,
      component: BoardView,
      activityBar: true,
      // Нав-действие — существующий экшен ui-стора (openBoard гасит плавающие/trap-слои, SWITCH_MAIN).
      activate: () => useUIStore.getState().openBoard(),
      isActive: (v) => v === 'board',
    });
  },
};
