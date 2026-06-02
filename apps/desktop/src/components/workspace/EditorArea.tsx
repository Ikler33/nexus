import { useTranslation } from 'react-i18next';
import { formatCombo } from '../../lib/commands';
import { useWorkspaceStore } from '../../stores/workspace';
import { GroupPane } from './GroupPane';
import styles from './EditorArea.module.css';

/** Область редактора: группы-сплиты в ряд (Б12). Пусто → подсказка. */
export function EditorArea() {
  const { t } = useTranslation();
  const groups = useWorkspaceStore((s) => s.groups);
  const hasContent = groups.some((g) => g.tabs.length > 0);

  if (!hasContent) {
    return (
      <p className={styles.hint}>
        {t('editor.selectFile', { shortcut: formatCombo('mod+o') })}
      </p>
    );
  }

  return (
    <div className={styles.groups}>
      {groups.map((g) => (
        <GroupPane key={g.id} groupId={g.id} />
      ))}
    </div>
  );
}
