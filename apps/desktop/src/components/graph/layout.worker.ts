import { computeLayout, type Positions } from './layout';
import type { GraphData } from '../../lib/tauri-api';

// Считаем раскладку графа вне main-thread (AC-PERF-6). `self` типизируем минимально,
// чтобы не тянуть webworker-lib (конфликт `self` с DOM-lib в общем tsconfig).
const ctx = self as unknown as {
  onmessage: ((e: MessageEvent<GraphData>) => void) | null;
  postMessage: (message: Positions) => void;
};

ctx.onmessage = (event) => {
  ctx.postMessage(computeLayout(event.data));
};
