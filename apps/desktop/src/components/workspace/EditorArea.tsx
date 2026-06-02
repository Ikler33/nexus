import { useWorkspaceStore } from '../../stores/workspace';
import { GroupPane } from './GroupPane';
import styles from './EditorArea.module.css';

/** Область редактора: группы-сплиты в ряд (Б12). Пусто → подсказка. */
export function EditorArea() {
  const groups = useWorkspaceStore((s) => s.groups);
  const hasContent = groups.some((g) => g.tabs.length > 0);

  if (!hasContent) {
    return <p className={styles.hint}>Выберите файл в дереве слева или нажмите Cmd/Ctrl+O</p>;
  }

  return (
    <div className={styles.groups}>
      {groups.map((g) => (
        <GroupPane key={g.id} groupId={g.id} />
      ))}
    </div>
  );
}
