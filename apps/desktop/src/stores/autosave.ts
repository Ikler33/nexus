// Автосейв буферов редактора (SAFE-4): дебаунс после паузы набора + немедленный flush на потере
// фокуса / смене вкладки / закрытии вкладки и окна. «Нетеряемость мысли» — правки сохраняются сами,
// без явного Ctrl-S. Таймеры держим в модульном Map (не в zustand-стейте — они не часть UI-состояния).
//
// Циклический импорт с workspace.ts безопасен: все обращения к стору — только в рантайме (внутри
// функций), на этапе инициализации модулей перекрёстных вызовов нет.
import { useWorkspaceStore } from './workspace';

/** Пауза набора до автосейва (мс). Аудит рекомендует 800–1500; берём 1000. */
const DEBOUNCE_MS = 1000;

const timers = new Map<string, ReturnType<typeof setTimeout>>();

function clear(path: string): void {
  const t = timers.get(path);
  if (t) {
    clearTimeout(t);
    timers.delete(path);
  }
}

/** Запланировать автосейв буфера через паузу набора. Повторный вызов сбрасывает таймер (debounce). */
export function scheduleAutosave(path: string): void {
  clear(path);
  timers.set(
    path,
    setTimeout(() => {
      timers.delete(path);
      void useWorkspaceStore.getState().saveBuffer(path);
    }, DEBOUNCE_MS),
  );
}

/** Немедленно сохранить буфер (если грязный) и отменить отложенный автосейв. */
export async function flush(path: string): Promise<void> {
  clear(path);
  const b = useWorkspaceStore.getState().buffers[path];
  if (b?.dirty) await useWorkspaceStore.getState().saveBuffer(path);
}

/** Сохранить ВСЕ грязные буферы (закрытие окна; фоновый sync FLOW переиспользует это). */
export async function flushAllDirty(): Promise<void> {
  const buffers = useWorkspaceStore.getState().buffers;
  const dirty = Object.keys(buffers).filter((p) => buffers[p].dirty);
  await Promise.all(dirty.map((p) => flush(p)));
}

/** Отменить запланированный автосейв БЕЗ сохранения (reset/тесты). */
export function cancelAutosave(path: string): void {
  clear(path);
}

/** Отменить ВСЕ запланированные автосейвы (reset воркспейса). */
export function cancelAllAutosave(): void {
  for (const t of timers.values()) clearTimeout(t);
  timers.clear();
}
