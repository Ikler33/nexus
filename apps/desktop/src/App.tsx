import { useEffect } from 'react';
import { FolderOpen } from 'lucide-react';
import { isTauri, tauriApi } from './lib/tauri-api';
import { useVaultStore } from './stores/vault';
import { Sidebar } from './components/sidebar/Sidebar';
import { Editor } from './components/editor/Editor';
import { BacklinksBar } from './components/editor/BacklinksBar';
import styles from './App.module.css';

/**
 * Каркас рабочего пространства (Ф0-3/Ф0-5): sidebar с деревом + редактор CodeMirror.
 * Вкладки/сплиты — Ф0-9, backlinks-бар — Ф0-6. Вне Tauri открывается мок-vault.
 */
export function App() {
  const info = useVaultStore((s) => s.info);
  const activeFile = useVaultStore((s) => s.activeFile);
  const dirty = useVaultStore((s) => s.dirty);
  const openVault = useVaultStore((s) => s.openVault);
  const setActiveContent = useVaultStore((s) => s.setActiveContent);
  const saveActiveFile = useVaultStore((s) => s.saveActiveFile);
  const openLink = useVaultStore((s) => s.openLink);

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
        <Sidebar />
      </aside>

      <main className={styles.main}>
        {activeFile ? (
          <div className={styles.editorPane}>
            <header className={styles.editorHeader}>
              <span className={styles.editorPath}>{activeFile.path}</span>
              {dirty && (
                <span className={styles.dirtyDot} title="Есть несохранённые изменения" aria-label="несохранено" />
              )}
            </header>
            <div className={styles.editorScroll}>
              <Editor
                path={activeFile.path}
                initialDoc={activeFile.content}
                onChange={setActiveContent}
                onSave={saveActiveFile}
                onOpenLink={openLink}
                getNotes={() => useVaultStore.getState().notes}
              />
            </div>
            <BacklinksBar />
          </div>
        ) : (
          <p className={styles.hint}>Выберите файл в дереве слева</p>
        )}
      </main>
    </div>
  );
}
