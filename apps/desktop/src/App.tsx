import { useEffect } from 'react';
import { FolderOpen } from 'lucide-react';
import { isTauri, tauriApi } from './lib/tauri-api';
import { useVaultStore } from './stores/vault';
import { FileTree } from './components/sidebar/FileTree';
import styles from './App.module.css';

/**
 * Каркас рабочего пространства (Ф0-3): sidebar с файловым деревом + основная область.
 * Полноценные вкладки/сплиты и редактор появятся в Ф0-5/Ф0-9. Вне Tauri открывается
 * мок-vault автоматически (превью/демо), в Tauri — через системный диалог выбора папки.
 */
export function App() {
  const info = useVaultStore((s) => s.info);
  const selectedPath = useVaultStore((s) => s.selectedPath);
  const openVault = useVaultStore((s) => s.openVault);

  useEffect(() => {
    if (!isTauri() && !info) {
      void openVault('');
    }
  }, [info, openVault]);

  const handleOpen = async () => {
    if (!isTauri()) {
      await openVault('');
      return;
    }
    const dir = await tauriApi.vault.pickDirectory();
    if (dir) await openVault(dir);
  };

  return (
    <div className={styles.layout}>
      <aside className={styles.sidebar}>
        <header className={styles.sidebarHeader}>
          <span className={styles.vaultName} title={info?.root}>
            {info?.name ?? 'Nexus'}
          </span>
          <button className={styles.openBtn} onClick={handleOpen} title="Открыть vault…">
            <FolderOpen size={16} aria-hidden />
          </button>
        </header>
        <FileTree />
      </aside>

      <main className={styles.main}>
        {selectedPath ? (
          <code className={styles.selected}>{selectedPath}</code>
        ) : (
          <p className={styles.hint}>Выберите файл в дереве слева</p>
        )}
      </main>
    </div>
  );
}
