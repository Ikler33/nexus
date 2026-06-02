import { useEffect, useState } from 'react';
import { isTauri, tauriApi } from './lib/tauri-api';
import styles from './App.module.css';

/**
 * Каркас приложения (Ф0-1). Контент-заглушка: проверяет, что фронт собирается,
 * рендерится и умеет ходить в нативный слой через единый IPC-шов `tauriApi`.
 * Реальный layout (sidebar / editor / AI-panel, DESIGN §3) появится в срезах Ф0-3+.
 */
export function App() {
  const [version, setVersion] = useState('dev');

  useEffect(() => {
    // В браузерном превью (без Tauri) IPC недоступен — остаётся 'dev'.
    if (!isTauri()) return;
    tauriApi.app
      .version()
      .then(setVersion)
      .catch(() => setVersion('dev'));
  }, []);

  return (
    <main className={styles.app}>
      <h1 className={styles.title}>Nexus</h1>
      <p className={styles.subtitle}>Local-first knowledge base · Phase 0 scaffold</p>
      <code className={styles.version}>v{version}</code>
    </main>
  );
}
