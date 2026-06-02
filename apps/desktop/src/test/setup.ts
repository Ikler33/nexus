import '@testing-library/jest-dom/vitest';

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

// Ненулевой размер вьюпорта, чтобы виртуализатор посчитал видимый диапазон.
Object.defineProperty(HTMLElement.prototype, 'offsetWidth', {
  configurable: true,
  get: () => 280,
});
Object.defineProperty(HTMLElement.prototype, 'offsetHeight', {
  configurable: true,
  get: () => 600,
});
