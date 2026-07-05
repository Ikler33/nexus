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

// ── F-1b: CI-граница «ядро/чужой-модуль ⇏ вырезанный модуль» ─────────────────────────────────────
// F-1 (выше) стережёт ТОЛЬКО кросс-импорты между зонами `components/<feature>`. Он НЕ ловит импорт
// вырезанного модуля из ЯДРА (App.tsx / stores / hooks / i18n / lib вне connector — всё ВНЕ
// `components/**`) и не запрещает манифесту тянуть ЧУЖОЙ модуль. F-10b-adversarial вскрыл: инвариант
// «ядро не импортирует модуль» держался grep-ом в ревью, НЕ в CI → будущий импорт ядро→модуль CI бы
// пропустил, изоляция тихо сломалась бы. Здесь инвариант ЗАКРЕПЛЁН в CI.
//
// Вырезанные модуль-фичи (F-9 news + F-10b 7 оверлеев). Каждый модуль = ПАРА артефактов:
//   • зона UI  `src/components/<mod>/**`            — компоненты фичи;
//   • манифест `src/lib/connector/modules/<mod>.ts` — проводка вклада в ядро через `ctx`.
// Обе можно импортировать ТОЛЬКО из самой этой пары (+ тест манифеста). Ядро получает вклад модуля
// через реестры коннектора (ctx.views/settings/commands/overlays/events), НЕ прямым импортом.
//
// F-10c (добавить новый вырезанный модуль в правило): допиши его имя строкой в MODULE_FEATURES —
// автоматически появятся и запрет его зоны/манифеста для ядра, и разрешение для его манифеста+теста.
// Ядру РАЗРЕШЕНО импортировать `lib/connector` (реестры/типы/ModuleContext/module-manager и барильер
// `modules` c `activateModules`) — это API коннектора, НЕ модуль; запрещены только `modules/<mod>`.
const MODULE_FEATURES = [
  'news',
  'goals',
  'memory',
  'episodes',
  'tasks',
  'inbox',
  'digest',
  'contradictions',
];

// Явные исключения границы модулей (аналог CROSS_IMPORT_WHITELIST для F-1): если shared-компонент
// ЧЕСТНО нужен ядру — задокументируй файл здесь с обоснованием, вместо ослабления правила глобально.
// Формат: { files, selfModule } — расширяет разрешение указанного файла на зону/манифест selfModule.
// Пусто: F-9/F-10b вырезали модули НАЧИСТО — ни один core/manifest не тянет чужой модуль.
const MODULE_BOUNDARY_EXCEPTIONS = [];

/** Паттерны no-restricted-imports для границы модулей.
 *  - selfModule: модуль, чьи ЗОНА `components/<mod>` и МАНИФЕСТ `modules/<mod>` импортировать МОЖНО
 *    (сам манифест и его тест); null — нельзя ни один.
 *  - allowManifests: true → любой манифест `modules/<mod>` разрешён (композиционный корень
 *    `modules/index.ts` регистрирует все); false — запрещён (кроме selfModule).
 *  - siblingManifests: включить относительные формы `./<mod>` / `../modules/<mod>` в запрет манифеста
 *    (ТОЛЬКО для файлов ВНУТРИ `modules/`, где `./<mod>` = манифест; в ядре `./<mod>` — чужой файл). */
function restrictModuleImports({
  selfModule = null,
  allowManifests = false,
  siblingManifests = false,
} = {}) {
  const patterns = [];
  for (const mod of MODULE_FEATURES) {
    if (mod !== selfModule) {
      patterns.push({
        group: [`**/components/${mod}`, `**/components/${mod}/**`],
        message:
          `Импорт вырезанного модуля components/${mod} запрещён вне самой фичи и её манифеста ` +
          `(F-1b, граница модуль/ядро): вклад модуля ядро получает через реестры коннектора ` +
          `(ctx.views/settings/commands/overlays/events), НЕ прямым импортом. Общее выноси в ` +
          `components/common/ или lib/. Легитимное исключение — MODULE_BOUNDARY_EXCEPTIONS.`,
      });
    }
    if (!allowManifests && mod !== selfModule) {
      // `**/connector/modules/<mod>` (без `lib/`) — чтобы ловить и относительные из `src/lib/…`,
      // где путь до манифеста НЕ содержит сегмента `lib` (напр. `./connector/modules/news`).
      const group = [`**/connector/modules/${mod}`];
      if (siblingManifests) group.push(`./${mod}`, `../modules/${mod}`);
      patterns.push({
        group,
        message:
          `Импорт манифеста модуля lib/connector/modules/${mod} запрещён (F-1b): модули независимы ` +
          `(общаются через ядро/ctx), а подключает манифесты ТОЛЬКО композиционный корень ` +
          `modules/index.ts (барильер lib/connector/modules — можно, это активатор, не манифест).`,
      });
    }
  }
  return patterns.length === 0 ? 'off' : ['error', { patterns }];
}

/** Компаньон для динамических import() (как F-1 §P2): no-restricted-imports НЕ проверяет
 *  ImportExpression — без этого правила границу обходил бы `await import('../components/<mod>/…')`. */
function restrictModuleDynamicImports({ selfModule = null, allowManifests = false } = {}) {
  const selectors = [];
  const bannedComponents = MODULE_FEATURES.filter((m) => m !== selfModule);
  if (bannedComponents.length > 0) {
    const alt = bannedComponents.join('|');
    selectors.push({
      selector: `ImportExpression > Literal[value=/components\\u002F(${alt})(\\u002F|$)/]`,
      message:
        'Динамический import() вырезанного модуля components/<mod> запрещён (F-1b) — тот же ' +
        'инвариант, что у статических; вклад модуля идёт через реестры коннектора.',
    });
  }
  const bannedManifests = allowManifests ? [] : MODULE_FEATURES.filter((m) => m !== selfModule);
  if (bannedManifests.length > 0) {
    const alt = bannedManifests.join('|');
    selectors.push({
      selector: `ImportExpression > Literal[value=/connector\\u002Fmodules\\u002F(${alt})(\\u002F|$)/]`,
      message:
        'Динамический import() манифеста модуля запрещён (F-1b): манифесты подключает только ' +
        'композиционный корень modules/index.ts.',
    });
  }
  return selectors.length === 0 ? 'off' : ['error', ...selectors];
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
  // ── F-1b: граница «ядро/чужой-модуль ⇏ вырезанный модуль» (список — MODULE_FEATURES) ───────────
  // ЯДРО = всё ВНЕ `src/components/**` и ВНЕ `src/lib/connector/modules/**` (App.tsx / stores /
  // hooks / i18n / lib вне connector): нельзя импортировать зону `components/<mod>` ни одного модуля
  // и нельзя тянуть конкретный манифест `modules/<mod>` (барильер `modules` — можно, это активатор).
  // Зоны `components/**` уже закрыты F-1 (кросс-фича выше), модуль-дир — блоками ниже; здесь — ядро.
  {
    files: ['**/*.{ts,tsx}'],
    ignores: ['src/components/**', 'src/lib/connector/modules/**'],
    rules: {
      'no-restricted-imports': restrictModuleImports({ selfModule: null, allowManifests: false }),
      'no-restricted-syntax': restrictModuleDynamicImports({
        selfModule: null,
        allowManifests: false,
      }),
    },
  },
  // FLOOR модуль-дир (adversarial F-1b): ядровой блок ИГНОРИРУЕТ `modules/**`, а блоки ниже матчат
  // ТОЛЬКО точные имена `<feature>.ts`/`<feature>.test.ts`/`index.ts`. Без этого floor ЛЮБОЙ другой
  // файл в `modules/` (стрэй-хелпер `news-helper.ts`; новый манифест `analytics.ts`, забытый в
  // MODULE_FEATURES) проваливался бы сквозь все блоки = 0 правил → laundering: манифест импортит
  // легальный на вид `./news-helper`, а тот свободно тянет чужую зону/манифест в обход границы.
  // Floor запрещает ВСЁ (selfModule:null); легит-случаи `<feature>.ts`/`index.ts` переопределяют его
  // блоками НИЖЕ (flat-config «последний матч по правилу побеждает»). siblingManifests — путь внутри
  // `modules/` до манифеста относительный (`./goals`), его тоже надо закрыть для стрэй-файлов.
  {
    files: ['src/lib/connector/modules/**/*.{ts,tsx}'],
    rules: {
      'no-restricted-imports': restrictModuleImports({
        selfModule: null,
        allowManifests: false,
        siblingManifests: true,
      }),
      'no-restricted-syntax': restrictModuleDynamicImports({ selfModule: null, allowManifests: false }),
    },
  },
  // Манифест модуля и его тест: разрешены СВОЯ зона `components/<mod>` и СВОЙ манифест `./<mod>`;
  // чужие зоны и чужие манифесты — запрещены (модули независимы, общаются через ядро/ctx).
  ...MODULE_FEATURES.map((feature) => ({
    files: [
      `src/lib/connector/modules/${feature}.ts`,
      `src/lib/connector/modules/${feature}.test.ts`,
    ],
    rules: {
      'no-restricted-imports': restrictModuleImports({
        selfModule: feature,
        allowManifests: false,
        siblingManifests: true,
      }),
      'no-restricted-syntax': restrictModuleDynamicImports({ selfModule: feature }),
    },
  })),
  // Композиционный корень `modules/index.ts`: подключает ВСЕ манифесты (allowManifests), но сам НЕ
  // импортирует зоны фич напрямую (вклад собирается из манифестов через реестры коннектора).
  {
    files: ['src/lib/connector/modules/index.ts'],
    rules: {
      'no-restricted-imports': restrictModuleImports({ selfModule: null, allowManifests: true }),
      'no-restricted-syntax': restrictModuleDynamicImports({ selfModule: null, allowManifests: true }),
    },
  },
  // Явные исключения границы модулей (пусто — см. MODULE_BOUNDARY_EXCEPTIONS): идут ПОСЛЕ зон и
  // расширяют разрешение только для указанного файла (аналог CROSS_IMPORT_WHITELIST для F-1).
  ...MODULE_BOUNDARY_EXCEPTIONS.map(({ files, selfModule }) => ({
    files: [files],
    rules: {
      'no-restricted-imports': restrictModuleImports({ selfModule, allowManifests: false }),
      'no-restricted-syntax': restrictModuleDynamicImports({ selfModule }),
    },
  })),
);
