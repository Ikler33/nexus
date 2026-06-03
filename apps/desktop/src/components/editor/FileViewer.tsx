import { convertFileSrc } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { isTauri } from '../../lib/tauri-api';
import { isPdf } from '../../lib/file-kind';
import { useVaultStore } from '../../stores/vault';
import styles from './FileViewer.module.css';

/**
 * Просмотр не-md вложений (Ф4-10): картинки и PDF во вкладке. URL файла — через asset-протокол
 * Tauri (`convertFileSrc`, разрешён CSP `img-src asset:`). Вне Tauri (браузер-превью) реальных
 * файлов нет → плейсхолдер. Inline-рендер в markdown (`![[embeds]]`/Mermaid/LaTeX) — эпик Live
 * Preview (BACKLOG).
 */
export function FileViewer({ path }: { path: string }) {
  const { t } = useTranslation();
  const root = useVaultStore((s) => s.info?.root);
  const name = path.slice(path.lastIndexOf('/') + 1);
  const url = isTauri() && root ? convertFileSrc(`${root}/${path}`) : null;

  if (!url) {
    return (
      <div className={styles.placeholder}>
        <span className={styles.name}>{name}</span>
        <span className={styles.hint}>{t('viewer.appOnly')}</span>
      </div>
    );
  }
  if (isPdf(path)) {
    return <iframe className={styles.pdf} src={url} title={name} />;
  }
  return (
    <div className={styles.imageWrap}>
      <img className={styles.image} src={url} alt={name} />
    </div>
  );
}
