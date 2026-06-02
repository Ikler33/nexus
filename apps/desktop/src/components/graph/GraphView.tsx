import { useEffect, useRef } from 'react';
import Graph from 'graphology';
import Sigma from 'sigma';
import { useTranslation } from 'react-i18next';
import { tauriApi } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import type { Positions } from './layout';
import styles from './GraphView.module.css';

function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

/**
 * Локальный N-hop граф активного файла (sigma.js + graphology, ADR-004). Раскладка
 * считается в Web Worker (`layout.worker.ts`), затем рендерится sigma (WebGL). Ленивая
 * загрузка (App рендерит только при открытии). Клик по узлу открывает файл.
 */
export default function GraphView() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeGraph);
  const center = useWorkspaceStore(activePath);
  const openFile = useWorkspaceStore((s) => s.openFile);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!center || !container) return;

    let sigma: Sigma | null = null;
    let worker: Worker | null = null;
    let cancelled = false;

    void (async () => {
      const data = await tauriApi.graph.getLocalGraph(center, 2);
      if (cancelled || !containerRef.current) return;

      const graph = new Graph();
      for (const n of data.nodes) {
        if (graph.hasNode(String(n.id))) continue;
        graph.addNode(String(n.id), {
          label: n.title ?? basename(n.path),
          path: n.path,
          x: 0,
          y: 0,
          size: n.path === center ? 9 : 5,
        });
      }
      for (const e of data.edges) {
        const s = String(e.source);
        const tt = String(e.target);
        if (graph.hasNode(s) && graph.hasNode(tt) && !graph.hasEdge(s, tt)) graph.addEdge(s, tt);
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
      worker.postMessage(data);
    })();

    return () => {
      cancelled = true;
      sigma?.kill();
      worker?.terminate();
    };
  }, [center, close, openFile]);

  return (
    <div className={styles.overlay} onClick={close}>
      <div
        className={styles.panel}
        role="dialog"
        aria-label={t('graph.title')}
        onClick={(e) => e.stopPropagation()}
      >
        {center ? (
          <div ref={containerRef} className={styles.canvas} />
        ) : (
          <p className={styles.empty}>{t('graph.empty')}</p>
        )}
      </div>
    </div>
  );
}
