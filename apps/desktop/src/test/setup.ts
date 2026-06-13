import '@testing-library/jest-dom/vitest';
import i18n from '../i18n/setup';

// Детерминируем локаль тестов (jsdom navigator.language = en): под ru написаны ассерты строк.
void i18n.changeLanguage('ru');

// --- Полифиллы/моки для @tanstack/react-virtual в jsdom ---
// virtual-core снимает размер контейнера через element.offsetWidth/offsetHeight (в jsdom = 0)
// и требует ResizeObserver. Без ненулевого размера виртуализатор не рендерит ни одной строки.

if (!('ResizeObserver' in globalThis)) {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  (globalThis as { ResizeObserver: unknown }).ResizeObserver = ResizeObserverStub;
}

if (!Element.prototype.scrollTo) {
  Element.prototype.scrollTo = () => {};
}

// CM6 при scrollIntoView (NAV-4: восстановление позиции курсора) мерит геометрию текста через
// Range.getClientRects/getBoundingClientRect — в jsdom их нет → «getClientRects is not a function»
// (async-ошибка после теста, валит coverage-прогон). В проде (Tauri-webview) методы есть; полифилл
// только для тестов: пустые прямоугольники CM6 обрабатывает как «не видно» без падения.
if (!Range.prototype.getClientRects) {
  Range.prototype.getClientRects = () =>
    ({
      length: 0,
      item: () => null,
      [Symbol.iterator]: function* () {},
    }) as unknown as DOMRectList;
}
if (!Range.prototype.getBoundingClientRect) {
  Range.prototype.getBoundingClientRect = () =>
    ({ top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
}

// Ненулевой размер вьюпорта, чтобы виртуализатор посчитал видимый диапазон.
Object.defineProperty(HTMLElement.prototype, 'offsetWidth', {
  configurable: true,
  get: () => 280,
});
Object.defineProperty(HTMLElement.prototype, 'offsetHeight', {
  configurable: true,
  get: () => 600,
});
