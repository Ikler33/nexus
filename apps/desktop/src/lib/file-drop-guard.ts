/**
 * Глобальный гард файловых дропов (NB-2 follow-up, adversarial-ревью CRITICAL).
 *
 * С `dragDropEnabled: false` в tauri.conf.json нативный file-drop Tauri отключён (иначе он глушит
 * HTML5 DnD в WKWebView) — но тогда дроп файла из Finder ВНЕ DOM-drop-зон уходит в дефолт WebKit:
 * навигацию webview на file:// с потерей SPA-состояния без «назад». Гард гасит дефолт ТОЛЬКО для
 * файловых drag'ов (`types` содержит 'Files'): внутренние DnD (карточки доски CARD_MIME, вкладки
 * TAB_MIME) не задевает — их типы без 'Files', предикат ложен. DOM-зоны файловых дропов (editor
 * image-drop, `lib/editor/imagePaste.ts`) отрабатывают раньше по target-фазе и сами делают
 * preventDefault; повторный preventDefault на window безвреден.
 *
 * Однократная установка из main.tsx (рядом с installErrorLog).
 */
export function installFileDropGuard(): void {
  const guard = (e: DragEvent) => {
    if (e.dataTransfer?.types.includes('Files')) e.preventDefault();
  };
  window.addEventListener('dragover', guard);
  window.addEventListener('drop', guard);
}
