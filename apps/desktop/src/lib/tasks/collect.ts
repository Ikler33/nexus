import { noteName } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { parseTasks } from '../editor/format';
import { tauriApi, type TaskItem } from '../tauri-api';

/**
 * Собирает все задачи vault (TASK-1), НАКЛАДЫВАЯ грязные открытые буферы поверх дискового списка
 * (источник правды для открытого-грязного файла — буфер, как в EDIT-5). Так дашборд показывает
 * несохранённые правки, а не устаревший диск. Чистые/закрытые файлы — из бэкенд-скана list_tasks.
 */
export async function collectTasks(): Promise<TaskItem[]> {
  const disk = await tauriApi.tasks.listTasks();
  const buffers = useWorkspaceStore.getState().buffers;
  const dirtyPaths = Object.keys(buffers).filter((p) => buffers[p].dirty);
  if (dirtyPaths.length === 0) return disk;

  // Заголовки из индекса (чтобы буферный оверлей не терял title); фолбэк — basename.
  const titleByPath = new Map<string, string | null>();
  for (const t of disk) if (!titleByPath.has(t.path)) titleByPath.set(t.path, t.title);

  const dirtySet = new Set(dirtyPaths);
  const kept = disk.filter((t) => !dirtySet.has(t.path));
  const fromBuffers: TaskItem[] = dirtyPaths.flatMap((path) => {
    const title = titleByPath.get(path) ?? noteName(path);
    return parseTasks(buffers[path].doc).map((t) => ({
      path,
      line: t.line,
      checked: t.checked,
      text: t.text,
      title,
    }));
  });
  return [...kept, ...fromBuffers];
}
