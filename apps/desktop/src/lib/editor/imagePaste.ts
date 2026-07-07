import { type Extension } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { tauriApi } from '../tauri-api';

/** Расширение файла по MIME картинки из буфера/перетаскивания. */
const EXT_BY_MIME: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/avif': 'avif',
  'image/bmp': 'bmp',
  'image/svg+xml': 'svg',
};

/** base64 (без `data:`-префикса) из Blob через FileReader — браузер кодирует сам. */
function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const s = String(reader.result);
      const comma = s.indexOf(',');
      resolve(comma >= 0 ? s.slice(comma + 1) : s);
    };
    reader.onerror = () => reject(reader.error ?? new Error('FileReader failed'));
    reader.readAsDataURL(blob);
  });
}

/** Сохраняет картинку в `attachments/` и вставляет `![](относительный/путь)` в позицию `at`. */
async function insertImage(view: EditorView, file: Blob, at: number): Promise<void> {
  try {
    const ext = EXT_BY_MIME[file.type] ?? 'png';
    // Date.now() + случайный суффикс: иначе мультидроп в одну мс перезаписал бы первый файл (data-loss).
    const name = `pasted-${Date.now()}-${Math.random().toString(36).slice(2, 8)}.${ext}`;
    const base64 = await blobToBase64(file);
    const rel = await tauriApi.attachments.write(name, base64);
    const md = `![](${rel})`;
    const pos = Math.min(at, view.state.doc.length);
    view.dispatch({ changes: { from: pos, to: pos, insert: md }, selection: { anchor: pos + md.length } });
    view.focus();
  } catch {
    // Запись не удалась (вне Tauri / ошибка бэка) — молча не вставляем (захват не теряем: можно
    // повторить). Без краша редактора.
  }
}

/**
 * Вставка/перетаскивание картинки в редактор (IMG-1): берёт image-файл из буфера обмена (Cmd-V) или
 * drag-drop, сохраняет в `attachments/<имя>` и вставляет markdown-ссылку `![](…)`. Не-image ВСТАВКА
 * (paste) проходит штатно (return false); не-image ФАЙЛОВЫЙ дроп глотается (NB-2: иначе builtin-drop
 * CodeMirror вставил бы содержимое файла как текст). Картинка показывается в превью (VaultImage).
 */
export function imagePaste(): Extension {
  return EditorView.domEventHandlers({
    paste(event, view) {
      const items = event.clipboardData?.items;
      if (!items) return false;
      for (const item of items) {
        if (item.kind === 'file' && item.type.startsWith('image/')) {
          const file = item.getAsFile();
          if (file) {
            event.preventDefault();
            void insertImage(view, file, view.state.selection.main.head);
            return true;
          }
        }
      }
      return false;
    },
    drop(event, view) {
      const files = event.dataTransfer?.files;
      const images = files ? Array.from(files).filter((f) => f.type.startsWith('image/')) : [];
      if (images.length === 0) {
        // Файловый дроп БЕЗ картинок — глотаем: иначе встроенный drop CodeMirror вставит СОДЕРЖИМОЕ
        // файла как текст (для бинарника — мусор в заметке). Актуально с dragDropEnabled:false (NB-2):
        // раньше нативный file-drop Tauri не пускал файлы до DOM. Не-файловые дропы (текст/выделение)
        // проходят штатно (return false).
        if (files && files.length > 0) {
          event.preventDefault();
          return true;
        }
        return false;
      }
      event.preventDefault();
      const start = view.posAtCoords({ x: event.clientX, y: event.clientY }) ?? view.state.selection.main.head;
      void (async () => {
        let at = start;
        for (const img of images) {
          await insertImage(view, img, at);
          at = view.state.selection.main.head; // курсор сдвинулся за вставленную ссылку
        }
      })();
      return true;
    },
  });
}
