import { useEffect, useRef, useState } from 'react';
import Graph from 'graphology';
import Sigma from 'sigma';
import { useTranslation } from 'react-i18next';
import { tauriApi } from '../../lib/tauri-api';
import type { FullGraph } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import type { Positions } from './layout';
import styles from './GraphView.module.css';

function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

type Mode = 'local' | 'full';

/** Лимит узлов единого графа: топ-N по связности (sigma+forceatlas2 в воркере тянет с запасом). */
const FULL_LIMIT = 2000;

interface Meta {
  shown: number;
  total: number;
  truncated: boolean;
}

/**
 * Граф ссылок (sigma.js + graphology, ADR-004). Два режима: **локальный** N-hop вокруг
 * активного файла и **единый** граф всего vault (топ-N по связности, AC-DOD-Ф3). Раскладка
 * всегда считается в Web Worker (`layout.worker.ts`) — main-thread не блокируется. Клик по
 * узлу открывает файл.
 */
export default function GraphView() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeGraph);
  const center = useWorkspaceStore(activePath);
  const openFile = useWorkspaceStore((s) => s.openFile);
  const containerRef = useRef<HTMLDivElement>(null);
  const [mode, setMode] = useState<Mode>('local');
  const [meta, setMeta] = useState<Meta | null>(null);

  const showCanvas = mode === 'full' || !!center;

  useEffect(() => {
    const container = containerRef.current;
    // Локальному режиму нужен активный файл; единому — нет.
    if ((mode === 'local' && !center) || !container) return;

    let sigma: Sigma | null = null;
    let worker: Worker | null = null;
    let cancelled = false;

    void (async () => {
      const data =
        mode === 'full'
          ? await tauriApi.graph.getFullGraph(FULL_LIMIT)
          : await tauriApi.graph.getLocalGraph(center!, 2);
      if (cancelled || !containerRef.current) return;

      if (mode === 'full') {
        const full = data as FullGraph;
        setMeta({ shown: full.nodes.length, total: full.totalFiles, truncated: full.truncated });
      } else {
        setMeta(null);
      }

      const graph = new Graph();
      for (const n of data.nodes) {
        if (graph.hasNode(String(n.id))) continue;
        graph.addNode(String(n.id), {
          label: n.title ?? basename(n.path),
          path: n.path,
          x: 0,
          y: 0,
          size: mode === 'local' && n.path === center ? 9 : 5,
        });
      }
      for (const e of data.edges) {
        const s = String(e.source);
        const tt = String(e.target);
        if (graph.hasNode(s) && graph.hasNode(tt) && !graph.hasEdge(s, tt)) graph.addEdge(s, tt);
      }
      // В едином графе масштабируем узлы по степени — хабы крупнее.
      if (mode === 'full') {
        graph.forEachNode((n) =>
          graph.setNodeAttribute(n, 'size', 3 + Math.min(8, graph.degree(n))),
        );
      }

      worker = new Worker(new URL('./layout.worker.ts', import.meta.url), { type: 'module' });
      worker.onmessage = (ev: MessageEvent<Positions>) => {
        if (cancelled || !containerRef.current) return;
        for (const [id, p] of Object.entries(ev.data)) {
          if (graph.hasNode(id)) {
            graph.setNodeAttribute(id, 'x', p.x);
            graph.setNodeAttribute(id, 'y', p.y);
          }
        }
        sigma = new Sigma(graph, containerRef.current);
        sigma.on('clickNode', ({ node }) => {
          const path = graph.getNodeAttribute(node, 'path') as string | undefined;
          if (path) {
            close();
            void openFile(path);
          }
        });
      };
      worker.postMessage({ nodes: data.nodes, edges: data.edges });
    })();

    return () => {
      cancelled = true;
      sigma?.kill();
      worker?.terminate();
    };
  }, [mode, center, close, openFile]);

  return (
    <div className={styles.overlay} onClick={close}>
      <div
        className={styles.panel}
        role="dialog"
        aria-label={t('graph.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <div className={styles.toolbar}>
          <span className={styles.title}>{t('graph.title')}</span>
          <div className={styles.modes} role="tablist" aria-label={t('graph.title')}>
            <button
              type="button"
              role="tab"
              aria-selected={mode === 'local'}
              className={`${styles.modeBtn} ${mode === 'local' ? styles.modeBtnActive : ''}`}
              onClick={() => setMode('local')}
            >
              {t('graph.modeLocal')}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={mode === 'full'}
              className={`${styles.modeBtn} ${mode === 'full' ? styles.modeBtnActive : ''}`}
              onClick={() => setMode('full')}
            >
              {t('graph.modeFull')}
            </button>
          </div>
          {mode === 'full' && meta && (
            <span className={styles.caption}>
              {meta.truncated
                ? t('graph.truncated', { shown: meta.shown, total: meta.total })
                : t('graph.nodeCount', { count: meta.shown })}
            </span>
          )}
        </div>
        <div className={styles.body}>
          {showCanvas ? (
            <div ref={containerRef} className={styles.canvas} />
          ) : (
            <p className={styles.empty}>{t('graph.empty')}</p>
          )}
        </div>
      </div>
    </div>
  );
}
