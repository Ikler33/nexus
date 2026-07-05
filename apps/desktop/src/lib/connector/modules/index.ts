/**
 * F-9 — ЕДИНСТВЕННАЯ точка регистрации модулей-вкладов поверх коннектора F-8 (композиционный корень
 * модулей). Здесь ядро подключает вырезанные модули: news — пилот; F-10-серия добавляет свои строкой
 * в `activateModules`. Импортируется РАДИ САЙД-ЭФФЕКТА из `App.tsx` (как `core-views`): регистрация +
 * активация происходят до первого рендера, чтобы ActivityBar / MainViewOutlet / SettingsView увидели
 * вклады. Идемпотентно (повторный импорт/HMR — no-op).
 *
 * Порядок активации детерминирован = порядку `register` (см. module-manager). Снятие всех вкладов —
 * `modules.disposeAll()` (тесты/HMR).
 */
import { modules } from '../module-manager';
import { episodesModule } from './episodes';
import { goalsModule } from './goals';
import { memoryModule } from './memory';
import { newsModule } from './news';
import { tasksModule } from './tasks';

let activated = false;

/** Регистрирует и активирует все модули-вклады фронта. Идемпотентно. */
export function activateModules(): void {
  if (activated) return;
  activated = true;
  modules.register(newsModule);
  // F-10b: оверлей-модули (вырезаны из core-overlays через `ctx.overlays`).
  modules.register(goalsModule);
  modules.register(memoryModule);
  modules.register(episodesModule);
  modules.register(tasksModule);
  modules.activateAll();
}

activateModules();
