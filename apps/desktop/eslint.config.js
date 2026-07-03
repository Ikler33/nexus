import js from '@eslint/js';
import globals from 'globals';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import tseslint from 'typescript-eslint';

// ── F-1: CI-линт границ модулей фронта («фича не импортирует фичу») ─────────────────────────────
// Каждый каталог src/components/<фича> — отдельная зона: импорт из ЧУЖОЙ фича-зоны запрещён.
// Разрешённые цели: components/common/, lib/, stores/ (пока), hooks/, i18n/. `common` — общий
// слой, не зона (но сам фичи импортировать тоже не может). Список выведен из фактической
// структуры src/components. Ядро-правило `no-restricted-imports` (плагина import в репо нет —
// зависимость не добавляем; все кросс-импорты здесь относительные `../<фича>/…`).
const FEATURE_DIRS = [
  'agent',
  'board',
  'chat',
  'chrome',
  'command',
  'contradictions',
  'digest',
  'editor',
  'episodes',
  'goals',
  'graph',
  'home',
  'inbox',
  'memory',
  'news',
  'onboarding',
  'plugins',
  'settings',
  'sidebar',
  'sync',
  'tasks',
  'today',
  'workspace',
];

// Честный ratchet (F-1): легитимные кросс-импорты, существовавшие ДО этого среза и НЕ разрываемые
// им, — явные исключения. Любой НОВЫЙ кросс-импорт — красный. Каждая запись рубится следующими
// F-срезами (распил вросших модулей editor/chat), после чего исключение удаляется.
const CROSS_IMPORT_WHITELIST = [
  {
    // Пик задачи рендерит содержимое заметки через MarkdownPreview, вросший в editor-зону —
    // рубится в F-X (вынос preview-рендера из editor).
    files: 'src/components/board/TaskPeek.tsx',
    zone: 'board',
    allow: ['editor'],
  },
  {
    // InspectorRail показывает chat/SuggestView (похожие заметки/предложения) — вросшая
    // editor↔chat нить, рубится в F-X.
    files: 'src/components/editor/InspectorRail.tsx',
    zone: 'editor',
    allow: ['chat'],
  },
  {
    // Workspace-панель хостит редакторную начинку (Editor/FileViewer/InlineAIBar/InspectorRail/
    // MentionsBar/TagSuggest/MarkdownPreviewHandle) — вросшая композиция, рубится в F-X.
    files: 'src/components/workspace/GroupPane.tsx',
    zone: 'workspace',
    allow: ['editor'],
  },
  {
    // MermaidDiagram перенесён в common (F-1) БЕЗ правки содержимого: его классы живут в
    // editor/MarkdownPreview.module.css — рубится в F-X (выделение mermaid-стилей из preview-CSS).
    files: 'src/components/common/MermaidDiagram.tsx',
    zone: 'common',
    allow: ['editor'],
  },
];

/** Опции no-restricted-imports: запрет всех фича-зон, кроме своей и явно разрешённых. */
function restrictFeatureImports(allowed) {
  const banned = FEATURE_DIRS.filter((f) => !allowed.includes(f));
  return [
    'error',
    {
      patterns: banned.map((f) => ({
        group: [`../${f}/**`, `**/components/${f}/**`],
        message:
          `Кросс-импорт фичи components/${f} запрещён (F-1, границы модулей): общее выноси в ` +
          `components/common/ или lib/. Легитимное старое ребро — только через ` +
          `CROSS_IMPORT_WHITELIST в eslint.config.js (ratchet: новых не добавлять).`,
      })),
    },
  ];
}

/** Компаньон для динамических `import()` (P2 ревью F-1): `no-restricted-imports` НЕ проверяет
 *  ImportExpression — без этого правила ratchet обходился одной строкой `await import('../x/…')`.
 *  Те же зоны/whitelist, что у статического правила (применяются парой). */
function restrictFeatureDynamicImports(allowed) {
  const banned = FEATURE_DIRS.filter((f) => !allowed.includes(f));
  if (banned.length === 0) return 'off';
  const alt = banned.join('|');
  const message =
    'Динамический import() кросс-фичи запрещён (F-1, границы модулей) — тот же ratchet, ' +
    'что у статических импортов; см. CROSS_IMPORT_WHITELIST.';
  return [
    'error',
    { selector: `ImportExpression > Literal[value=/^\\.\\.\\u002F(${alt})\\u002F/]`, message },
    { selector: `ImportExpression > Literal[value=/components\\u002F(${alt})\\u002F/]`, message },
  ];
}

export default tseslint.config(
  { ignores: ['dist', 'src-tauri', 'coverage', 'e2e/playwright-report', 'e2e/test-results'] },
  {
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      'react-refresh/only-export-components': [
        'warn',
        { allowConstantExport: true },
      ],
    },
  },
  // Фича-зоны (F-1): по блоку на зону; своя зона разрешена, чужие — нет.
  ...FEATURE_DIRS.map((feature) => ({
    files: [`src/components/${feature}/**/*.{ts,tsx}`],
    rules: {
      'no-restricted-imports': restrictFeatureImports([feature]),
      'no-restricted-syntax': restrictFeatureDynamicImports([feature]),
    },
  })),
  // common — общий слой: сам не импортирует ни одну фича-зону.
  {
    files: ['src/components/common/**/*.{ts,tsx}'],
    rules: {
      'no-restricted-imports': restrictFeatureImports([]),
      'no-restricted-syntax': restrictFeatureDynamicImports([]),
    },
  },
  // Точечные исключения ratchet'а — идут ПОСЛЕ зон и сужают запрет только для указанного файла.
  ...CROSS_IMPORT_WHITELIST.map(({ files, zone, allow }) => ({
    files: [files],
    rules: {
      'no-restricted-imports': restrictFeatureImports(
        zone === 'common' ? allow : [zone, ...allow],
      ),
      'no-restricted-syntax': restrictFeatureDynamicImports(
        zone === 'common' ? allow : [zone, ...allow],
      ),
    },
  })),
);
