import { useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { renderMermaid, type MermaidTheme } from '../../lib/markdown/mermaid';
import { isDarkTheme, useThemeStore } from '../../stores/theme';
import styles from './MarkdownPreview.module.css';

/**
 * Mermaid-диаграмма в режиме чтения (Live-Preview). Лениво рендерит mermaid → CSP-безопасный SVG
 * (`renderMermaid` → `cspSafeSvg`: стили в presentation-атрибуты, без `<style>`/`<script>`/`on*`/
 * `javascript:`/SMIL), вставляет через `dangerouslySetInnerHTML` (вход уже санитизирован). Тема mermaid
 * следует теме приложения (тёмная/светлая) и ПЕРЕРЕНДЕРИВАЕТ диаграмму при переключении (ревью: иначе
 * цвета зашиты под светлую). Ошибка синтаксиса / непарс — заглушка вместо краша.
 */
export function MermaidDiagram({ code }: { code: string }) {
  const { t } = useTranslation();
  const appTheme = useThemeStore((s) => s.theme);
  const rawId = useId();
  const [state, setState] = useState<'loading' | 'ready' | 'error'>('loading');
  const [svg, setSvg] = useState('');

  useEffect(() => {
    let alive = true;
    setState('loading');
    // id mermaid'а — валидный для DOM/CSS (без `:` из useId). ВСЕ тёмные темы (канон DARK_THEMES) →
    // mermaid 'dark' (раньше — только dark/midnight, новые тёмные темы Qasr рендерили светлый mermaid).
    const id = `mmd-${rawId.replace(/[^a-zA-Z0-9_-]/g, '')}`;
    const theme: MermaidTheme = isDarkTheme(appTheme) ? 'dark' : 'default';
    void renderMermaid(code, id, theme)
      .then((s) => {
        if (alive) {
          setSvg(s);
          setState('ready');
        }
      })
      .catch(() => {
        // mermaid на ошибке синтаксиса бросает ДО уборки своего temp-узла `d<id>` (ревью) — чистим сами.
        document.getElementById(`d${id}`)?.remove();
        if (alive) setState('error');
      });
    return () => {
      alive = false;
    };
  }, [code, rawId, appTheme]);

  if (state === 'error') return <div className={styles.embedNote}>{t('mermaid.error')}</div>;
  if (state === 'loading') return <div className={styles.embedNote}>{t('mermaid.loading')}</div>;
  // svg уже CSP-безопасен и санитизирован (cspSafeSvg) — единственный осознанный dangerouslySetInnerHTML.
  return <div className={styles.mermaid} dangerouslySetInnerHTML={{ __html: svg }} />;
}
