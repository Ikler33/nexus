import { beforeAll, describe, expect, it } from 'vitest';

import { installFileDropGuard } from './file-drop-guard';

/** Drag-событие с заданными `dataTransfer.types` (jsdom не даёт конструктора DragEvent —
 *  собираем Event + defineProperty, как в BoardView-тестах). */
function dragEvent(type: 'dragover' | 'drop', types: string[] | null): Event {
  const e = new Event(type, { bubbles: true, cancelable: true });
  if (types) Object.defineProperty(e, 'dataTransfer', { value: { types } });
  return e;
}

describe('file-drop-guard (NB-2: dragDropEnabled:false → гард против file://-навигации)', () => {
  beforeAll(() => {
    installFileDropGuard();
  });

  it('drop с файлами (types содержит Files) → default погашен (нет навигации на file://)', () => {
    const e = dragEvent('drop', ['Files']);
    window.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(true);
  });

  it('dragover с файлами → default погашен (окно — валидная drop-цель, дроп не отдаётся ОС)', () => {
    const e = dragEvent('dragover', ['Files']);
    window.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(true);
  });

  it('внутренний DnD (CARD_MIME, без Files) → гард НЕ вмешивается', () => {
    const e = dragEvent('drop', ['application/x-nexus-board-card']);
    window.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
  });

  it('drop без dataTransfer → no-op без падения', () => {
    const e = dragEvent('drop', null);
    window.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
  });
});
