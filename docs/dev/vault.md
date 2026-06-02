# Vault-слой — `src-tauri/src/vault` + стор/дерево фронта

> Подсистема Ф0-3. Безопасность путей — §7.4/§11 (**AC-SEC-1**). Ленивое дерево — §4.1/§10 (**AC-PERF-7**).

## Назначение
Доступ к файловой системе хранилища: ленивый листинг каталогов и единая канонизация путей.
Открытие vault кладёт `root` + `Database` в managed state приложения.

## Rust
- **`resolve_vault_path(root, rel)`** — ЕДИНСТВЕННАЯ точка анти-traversal: блокирует
  абсолютные пути и побег за пределы vault (`..`, симлинки) через `canonicalize` +
  `starts_with(root)`. Все vault/host-команды резолвят пути только так (AC-SEC-1).
  `root` канонизируется в `open_vault`.
- **`list_dir(root, rel)`** — содержимое ОДНОГО каталога (`''` = корень); скрывает
  игнорируемое (`is_ignored`: dotfiles, `.conflict`); вложенное НЕ раскрывает (ленивость —
  не 50k одним IPC). `FileEntry { name, path('/'-разделитель), isDir, hasChildren, sizeBytes }`.
- **Команды** (`commands/vault.rs`): `open_vault(path) -> VaultInfo`,
  `list_dir(dirPath) -> Vec<FileEntry>` (ФС-обход в `spawn_blocking`).
  State: `AppState { vault: RwLock<Option<VaultContext { root, db }>> }`.

## Фронт
- IPC-шов `lib/tauri-api.ts`: `vault.openVault / listDir / pickDirectory`; вне Tauri —
  мок (`lib/mock/vault.ts`), чтобы вести фронт на контрактах без бэкенда (DESIGN §0).
- Стор `stores/vault.ts` (Zustand): `childrenByPath`, `expanded`, `loading`, `selectedPath`;
  `flattenVisible()` — плоский список видимых узлов для виртуализации.
- `components/sidebar/FileTree.tsx`: `@tanstack/react-virtual` (рендер только видимой
  области), клавиатура ↑/↓/→/←/Enter через `aria-activedescendant`, ARIA `tree`/`treeitem`.

## Тесты
- Rust: листинг (скрытие ignored, ленивость, относительный путь), `resolve_vault_path`
  (traversal/абсолют отклонены) → **AC-SEC-1**.
- Фронт: стор (ленивая загрузка/раскрытие/глубина), FileTree (рендер, клик-раскрытие, выбор).

## Инварианты / дальше
- Пути наружу команд — относительные, `/`; абсолютные и `..` отклоняются до ФС-доступа.
- Сортировка узлов — на фронте через `Intl.Collator` (Ф0-10); сейчас порядок ФС.
- Фильтр дерева по индексу/FTS — после индексатора (Ф0-4/Ф0-7).
- `resolve_vault_path` пока требует существования пути; вариант для write-путей
  (несуществующий файл → канонизация родителя) добавится с write-командами.
