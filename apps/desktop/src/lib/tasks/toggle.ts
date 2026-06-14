import { useWorkspaceStore } from '../../stores/workspace';
import { toggleTaskAtLine } from '../editor/format';
import { tauriApi } from '../tauri-api';

/**
 * Тоггл задачи на месте из дашборда (TASK-1) — БУФЕР-AWARE. Открытый буфер (возможно грязный) —
 * источник правды: пишем через updateBufferDoc (как EDIT-5 → dirty + debounced-автосейв + flush SAFE-4),
 * чтобы не разойтись с тем, что пользователь редактирует. Закрытый файл: читаем диск, тоггл, атомарная
 * запись (manual=false → как автосейв). Возврат false = строка уже не таск (дрейф номера строки между
 * загрузкой дашборда и кликом) → вызывающий перезагружает список, а не молча проглатывает.
 */
export async function toggleTaskInPlace(path: string, line: number): Promise<boolean> {
  const ws = useWorkspaceStore.getState();
  const buf = ws.buffers[path];
  if (buf) {
    const next = toggleTaskAtLine(buf.doc, line);
    if (next == null) return false;
    ws.updateBufferDoc(path, next);
    return true;
  }
  try {
    const meta = await tauriApi.vault.readFileMeta(path);
    const next = toggleTaskAtLine(meta.content, line);
    if (next == null) return false;
    await tauriApi.vault.writeFile(path, next, false);
    return true;
  } catch {
    return false; // файл исчез/нет доступа — мягкая ошибка, UI перезагрузит
  }
}
