// AI-1 (A1, спека `docs/specs/kanban-board.md` §10): «На доску» — продвижение заметки в задачу канбана
// БЕЗ LLM, чистым frontmatter-контрактом. Заметка без `status` → задача в первой колонке дефолт-доски
// (`personal`); статус-ключ и набор колонок берём из ЕЁ конфига (уважаем кастомизацию владельца, §11).
// Если у заметки уже есть непустой `status` — она уже задача: НЕ перетираем колонку (иначе «На доску»
// откатил бы её из doing/done в первую колонку = потеря состояния, §12). Логика отделена от UI (тостов/
// открытия доски) — юнит-тестируема.

import { flushBufferIfDirty, writeFrontmatterField } from './frontmatter-edit';
import { tauriApi } from './tauri-api';

/** Исход промоута: `promoted` — статус проставлен; `already` — заметка уже задача (её текущая колонка). */
export type PromoteOutcome =
  | { kind: 'promoted'; statusKey: string; column: string }
  | { kind: 'already'; statusKey: string; column: string };

/** Резолвит заметку на доску. `column` всегда = id колонки (для `already` — текущее raw-значение status,
 *  для `promoted` — id первой колонки конфига). Бросает (как `writeFrontmatterField`) при сбое флаша/записи —
 *  вызывающий ловит и тостит. */
export async function promoteToBoard(path: string): Promise<PromoteOutcome> {
  // Дефолт-доска `personal`: источник истины для статус-ключа и первой колонки (учитывает кастом-колонки).
  const board = await tauriApi.board.get();
  const statusKey = board.config.statusKey.trim() || 'status';
  const column = board.config.columns[0]?.id ?? 'todo';
  // Флашим грязный буфер ДО чтения status: forNote читает ДИСК, а не открытый буфер. Без этого только что
  // набранный (несохранённый) `status: doing` не виден guard'у → запись откатила бы его в первую колонку
  // (data-loss класса BOARD-5 R1). Сбой флаша → FlushFailedError, ничего не пишем (ловит вызывающий).
  await flushBufferIfDirty(path);
  // Текущий status заметки (PROP-2 forNote — скаляры frontmatter). Непустой → уже задача, не трогаем.
  const props = await tauriApi.properties.forNote(path);
  const cur = props.find((p) => p.key === statusKey);
  if (cur && cur.value.trim() !== '') {
    return { kind: 'already', statusKey, column: cur.value.trim() };
  }
  await writeFrontmatterField(path, statusKey, column);
  return { kind: 'promoted', statusKey, column };
}
