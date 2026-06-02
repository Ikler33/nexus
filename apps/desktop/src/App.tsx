import { useEffect } from 'react';
import { FolderOpen } from 'lucide-react';
import { isTauri } from './lib/tauri-api';
import { openVaultFlow, registerCoreCommands } from './lib/commands-core';
import { useKeymap } from './hooks/useKeymap';
import { useVaultStore } from './stores/vault';
import { Sidebar } from './components/sidebar/Sidebar';
import { EditorArea } from './components/workspace/EditorArea';
import { CommandPalette } from './components/command/CommandPalette';
import styles from './App.module.css';

/**
 * Оболочка приложения: sidebar (поиск + дерево) + область редактора со вкладками/сплитами
 * (Б12) + Command Palette. Вне Tauri открывается мок-vault. Глобальные хоткеи — через keymap.
 */
export function App() {
  const info = useVaultStore((s) => s.info);

  useKeymap();

  useEffect(() => {
    const disposable = registerCoreCommands();
    return () => disposable.dispose();
  }, []);

  useEffect(() => {
    if (!isTauri() && !info) {
      void openVaultFlow();
    }
  }, [info]);

  return (
    <div className={styles.layout}>
      <aside className={styles.sidebar}>
        <header className={styles.sidebarHeader}>
          <span className={styles.vaultName} title={info?.root}>
            {info?.name ?? 'Nexus'}
          </span>
          <button
            className={styles.openBtn}
            onClick={() => void openVaultFlow()}
            title="Открыть vault…"
          >
            <FolderOpen size={16} aria-hidden />
          </button>
        </header>
        <Sidebar />
      </aside>

      <main className={styles.main}>
        <EditorArea />
      </main>

      <CommandPalette />
    </div>
  );
}
