import { lazy, Suspense } from 'react';
import './graph.css';

/**
 * F-10d — слой-обёртка графа: сохраняет прежнюю DOM/CSS-семантику из App.tsx (ленивый `GraphView`
 * под `Suspense` внутри `div.graph-layer` — `position:absolute; inset:0` В ГРАНИЦАХ `.appBody`, НЕ
 * поверх титлбара/статусбара — фикс владельца). При вырезе graph в оверлей-модуль (F-10d) этот
 * колокейт переезжает из ядра (App.tsx) в graph-зону: `OverlayOutlet` (mount:'appBody') рендерит
 * ИМЕННО его как компонент оверлея — outlet остаётся дословно generic (лишь фильтр по mount).
 *
 * Порядок обёрток дословно как в прежнем App.tsx: `Suspense` СНАРУЖИ `div.graph-layer` (пока чанк
 * `GraphView` грузится — fallback `null`, слоя нет), ErrorBoundary даёт сам outlet (per-contribution).
 * `GraphView` остаётся ленивым (тяжёлый d3-force/louvain §10) — импорт `graph.css` здесь лишь тянет
 * лёгкий стиль слоя (класс `.graph-layer`), тяжёлый JS графа по-прежнему в отдельном чанке.
 */
const GraphView = lazy(() => import('./GraphView'));

export function GraphLayer() {
  return (
    <Suspense fallback={null}>
      <div className="graph-layer">
        <GraphView />
      </div>
    </Suspense>
  );
}
