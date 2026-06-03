import { activeBuffer, useWorkspaceStore } from '../stores/workspace';
import { isViewable } from './file-kind';

/**
 * Печать / экспорт PDF активной заметки (Ф4-13): рендерит её в чистый print-контейнер и вызывает
 * системный диалог печати (там же «Сохранить как PDF»). Печатает **исходник markdown**; отрендеренный
 * HTML / Mermaid / LaTeX — эпик Live Preview (BACKLOG). Оболочка (titlebar/sidebar/…) скрыта через
 * `@media print` (см. styles.css). Контейнер удаляется по событию `afterprint`.
 */
export function printActiveNote(): void {
  if (typeof document === 'undefined' || typeof window === 'undefined') return;
  const buf = activeBuffer(useWorkspaceStore.getState());
  if (!buf || isViewable(buf.path)) return;

  const root = document.createElement('div');
  root.className = 'nexus-print-root';
  const title = document.createElement('h1');
  title.textContent = buf.path.slice(buf.path.lastIndexOf('/') + 1);
  const body = document.createElement('pre');
  body.textContent = buf.doc;
  root.append(title, body);
  document.body.appendChild(root);

  const cleanup = () => {
    root.remove();
    window.removeEventListener('afterprint', cleanup);
  };
  window.addEventListener('afterprint', cleanup);
  window.print();
}
