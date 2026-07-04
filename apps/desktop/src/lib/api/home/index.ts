import * as mockHome from '../../mock/home';
import * as mockVault from '../../mock/vault';
import { bridge, subscribe } from '../bridge';
import type {
  GoalEntry,
  HomeActivity,
  HomeData,
  OpenQuestion,
  StaleNote,
  Widget,
} from './types';

/**
 * Home-домен (F-2d): HOME-дашборд (бэкенд H1/H2/H6, страница DP-1) — статические данные, активность,
 * кэшированные LLM-виджеты (daily brief / open questions / context drift / stale radar), тоггл
 * «Инсайты» + подписка на обновление виджета (`home:widget-updated`). Плюс заметки-цели (#35) — часть
 * `HomeData`, поэтому живут в этом же домене. Все вызовы — через `bridge` (Tauri ↔ мок `lib/mock/*`);
 * потребители ходят сюда по-прежнему через `tauriApi.home`/`tauriApi.goals`/`tauriApi.events`
 * (barrel-реэкспорт в `lib/tauri-api.ts`). Вне Tauri — стейтфул-мок с контентом макета.
 */
export const home = {
  /** Статические/динамические данные HOME (stats/recent/goals) одним запросом, без LLM. */
  data: (): Promise<HomeData> => bridge<HomeData>('get_home_data', undefined, () => mockHome.data()),

  /** Зона «Активность» (H6): heatmap правок, серия дней, сироты, «Продолжить» со сниппетом.
   *  `tzOffsetMin` = `new Date().getTimezoneOffset()` — дни считаются в локали пользователя. */
  activity: (): Promise<HomeActivity> =>
    bridge<HomeActivity>(
      'get_home_activity',
      { tzOffsetMin: new Date().getTimezoneOffset() },
      () => mockHome.activity(),
    ),

  /** Кэшированный LLM-виджет по ключу (или `null`, если ещё не генерировался). Мгновенно — НЕ ждёт
   *  LLM (генерация фоном; готовность — событие `home:widget-updated`). H2. Известные ключи:
   *  `'daily_brief'` (H3, зона 2), `'open_questions'` (H5, зона 4, manual), `'context_drift'`
   *  (H5, зона 5, scheduled). Для последних двух есть типизированные хелперы ниже. */
  widget: (key: string): Promise<Widget | null> =>
    bridge<Widget | null>('get_widget', { key }, () => mockHome.widget(key)),

  /** Ручной refresh виджета (manual): ставит фоновую генерацию в очередь (требует зарегистрированный
   *  виджет; дедуп активной джобы). Завершение — событие `home:widget-updated`. H2. */
  refresh: (key: string): Promise<void> =>
    bridge<void>('refresh_widget', { key }, () => mockHome.refresh(key)),

  /** «Stale radar» (H4, зона 4): ранжированный список устаревших заметок. Слой 1 (скоринг) мгновенно
   *  on-open; слой 2 (LLM-причина/действие/подсказка) — из кэша, если обогащали. */
  staleRadar: (): Promise<StaleNote[]> =>
    bridge<StaleNote[]>('get_stale_radar', undefined, () => mockHome.staleRadar()),

  /** Ручной запуск LLM-обогащения «Stale radar» (слой 2, manual): топ-N → причина/действие/подсказка,
   *  кэш 24ч. Требует chat; дедуп активной джобы. Завершение — событие `home:widget-updated`
   *  (ключ `'stale_radar'`). Вне Tauri — no-op. */
  staleRefresh: (): Promise<void> =>
    bridge<void>('refresh_stale_radar', undefined, () => mockHome.staleRefresh()),

  /** Состояние тоггла «Инсайты» (проактивные ИИ-виджеты Home: открытые вопросы + дрейф контекста +
   *  stale-radar). Persisted, дефолт OFF. Вне Tauri — мок. */
  insightsGetEnabled: (): Promise<boolean> =>
    bridge<boolean>('insights_get_enabled', undefined, () => mockHome.insightsGetEnabled()),

  /** Переключить «Инсайты»; при включении бэкенд ставит kick-джобы доступных виджетов. Вне Tauri — мок. */
  insightsSetEnabled: (on: boolean): Promise<void> =>
    bridge<void>('insights_set_enabled', { on }, () => mockHome.insightsSetEnabled(on)),

  /** «Open questions» (H5, зона 4, manual): незакрытые вопросы из последних заметок — распарсенный
   *  контент виджета `open_questions`. Сгенерировать/обновить — `home.refresh('open_questions')`;
   *  готовность — событие `onWidgetUpdated`. Пока не сгенерировано — `[]`. */
  openQuestions: async (): Promise<OpenQuestion[]> => {
    const w = await home.widget('open_questions');
    if (!w?.content) return [];
    try {
      return JSON.parse(w.content) as OpenQuestion[];
    } catch {
      return [];
    }
  },

  /** «Context drift» (H5, зона 5, scheduled): абзац расхождения «текущий фокус vs цели» — контент
   *  виджета `context_drift` (или `null`, если ещё не сгенерировано/пусто). Обновляется раз в сутки
   *  в фоне; принудительно — `home.refresh('context_drift')`. */
  contextDrift: async (): Promise<string | null> => {
    const w = await home.widget('context_drift');
    return w?.content ? w.content : null;
  },
};

/** Заметки-цели (#35, часть HOME-дашборда). Офлайн, без LLM. Вне Tauri — мок. */
export const goals = {
  /** Все заметки-цели (инлайн-тег `#goal`) с прогрессом (#35). Офлайн, без LLM. Вне Tauri — мок. */
  list: (): Promise<GoalEntry[]> => bridge<GoalEntry[]>('list_goals', undefined, () => mockVault.getGoals()),
};

/** Событийная подписка home-домена. Вне Tauri — no-op (мок-бэкенд виджеты не генерирует). */
export const homeEvents = {
  /**
   * Подписка на «HOME-виджет обновился» (backend `emit("home:widget-updated", key)` после записи кэша
   * виджета — H2). Колбэк получает ключ виджета → фронт перечитывает его `tauriApi.home.widget(key)`.
   * Возвращает функцию отписки. Вне Tauri — no-op (мок-бэкенд не генерирует виджеты).
   */
  onWidgetUpdated: (cb: (key: string) => void): Promise<() => void> =>
    subscribe<string>('home:widget-updated', cb),
};
