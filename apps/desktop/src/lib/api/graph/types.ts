/**
 * DTO-типы graph-домена (F-2d): беклинки, незалинкованные упоминания, узлы/рёбра локального и
 * единого графа vault. Зеркала Rust-структур (`graph::*`) — контракт провода `invoke`. Потребители
 * импортируют их по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Обратная ссылка (зеркалит Rust `graph::BacklinkEntry`). */
export interface BacklinkEntry {
  sourcePath: string;
  sourceTitle: string | null;
  context: string | null;
  lineNumber: number | null;
}

/** Незалинкованное упоминание (зеркалит Rust `graph::MentionEntry`). */
export interface MentionEntry {
  sourcePath: string;
  sourceTitle: string | null;
  snippet: string;
}

/** Узел/ребро/данные локального графа (зеркалит Rust `graph::*`). */
export interface GraphNode {
  id: number;
  path: string;
  title: string | null;
  /** Теги заметки (без `#`, отсортированы) — цвет узла и фильтр-чипы графа. */
  tags: string[];
}
export interface GraphEdge {
  source: number;
  target: number;
}
export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
}
/** Единый граф всего vault (зеркалит Rust `graph::FullGraph`). */
export interface FullGraph {
  nodes: GraphNode[];
  edges: GraphEdge[];
  /** Всего не-удалённых файлов в vault. */
  totalFiles: number;
  /** Показаны не все узлы (обрезано по степени связности). */
  truncated: boolean;
}
