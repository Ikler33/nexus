import { useEffect } from 'react';
import { FolderOpen, Languages } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { isTauri } from './lib/tauri-api';
import { openVaultFlow, registerCoreCommands } from './lib/commands-core';
import { useKeymap } from './hooks/useKeymap';
import { changeLocale } from './i18n/setup';
import { useVaultStore } from './stores/vault';
import { Sidebar } from './components/sidebar/Sidebar';
import { EditorArea } from './components/workspace/EditorArea';
import { CommandPalette } from './components/command/CommandPalette';
import styles from './App.module.css';

/**
 * Оболочка приложения: sidebar (поиск + дерево) + область редактора со вкладками/сплитами
 * (Б12) + Command Palette. Вне Tauri открывается мок-vault. Хоткеи — через keymap. i18n RU/EN.
 */
export function App() {
  const info = useVaultStore((s) => s.info);
  const { t, i18n } = useTranslation();

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
            {info?.name ?? t('app.name')}
          </span>
          <div className={styles.headerActions}>
            <button
              className={styles.openBtn}
              onClick={() => changeLocale(i18n.language === 'ru' ? 'en' : 'ru')}
              title="Язык / Language"
              aria-label="Язык / Language"
            >
              <Languages size={16} aria-hidden />
            </button>
            <button
              className={styles.openBtn}
              onClick={() => void openVaultFlow()}
              title={t('app.openVault')}
              aria-label={t('app.openVault')}
            >
              <FolderOpen size={16} aria-hidden />
            </button>
          </div>
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
