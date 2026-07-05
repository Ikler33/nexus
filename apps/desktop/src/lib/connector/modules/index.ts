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
import { newsModule } from './news';

let activated = false;

/** Регистрирует и активирует все модули-вклады фронта. Идемпотентно. */
export function activateModules(): void {
  if (activated) return;
  activated = true;
  modules.register(newsModule);
  // F-10: следующие вырезанные модули регистрируются здесь (одной строкой на модуль).
  modules.activateAll();
}

activateModules();
