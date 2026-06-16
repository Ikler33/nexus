// AI-2c: применение принятых авто-тегов к заметке. Запись — инлайн `#tag` в ТЕЛО (frontmatter-список
// недоступен: `set_frontmatter_field` round-trip-режектит `[...]`; собирать YAML-список вручную — вернуть
// тот самый класс порчи, ради которого скаляр-примитив и сделан). Инлайн-теги индексатор подхватывает тем
// же путём, что любой набранный руками `#tag`. Данные-безопасность: флаш грязного буфера ДО записи
// (урок AI-1 R1: writeFile целит диск, несохранённый буфер затёрся бы), идемпотентность (уже-присутствующие
// теги не дублируем), анти-эхо SAFE-3 после записи.

import { flushBufferIfDirty } from './frontmatter-edit';
import { tauriApi } from './tauri-api';
import { useWorkspaceStore } from '../stores/workspace';

/** Tag-символы: те же, что у индексатора (буквы/цифры/`_`/`-`/`/`, Unicode). */
const TAG_TOKEN = /(?:^|\s)#([\p{L}\p{N}_/-]+)/gu;

/** Инлайн-теги, УЖЕ присутствующие в тексте (нормализованные lowercase, без `#`). Зеркалит инвариант
 *  индексатора (`parser::scan_wiki_and_tags`): тег обязан содержать ≥1 БУКВУ — `#2024` тегом НЕ считается
 *  (иначе мы бы зря отсеивали валидное предложение). Frontmatter `tags:`-список здесь не виден (тело-скан) —
 *  возможный дубль frontmatter↔inline безвреден (append-only, индекс дедупит) — в BACKLOG. */
export function existingInlineTags(content: string): Set<string> {
  const out = new Set<string>();
  for (const m of content.matchAll(TAG_TOKEN)) {
    if (/\p{L}/u.test(m[1])) out.add(m[1].toLowerCase()); // как индексатор: нужна ≥1 буква
  }
  return out;
}

/** Чистая раскладка записи: дописывает НЕДОСТАЮЩИЕ теги одной инлайн-строкой в конец тела. Возвращает
 *  новый контент + реально добавленные (нормализованные). Идемпотентно: уже-присутствующие/дубли в
 *  аргументе пропускаются; добавлять нечего → контент НЕ меняется (без лишних снапшотов истории). */
export function appendInlineTags(
  content: string,
  tags: string[],
): { content: string; added: string[] } {
  const present = existingInlineTags(content);
  const seen = new Set<string>();
  const added: string[] = [];
  for (const raw of tags) {
    const t = raw.trim().replace(/^#/, '').toLowerCase();
    if (!t || present.has(t) || seen.has(t)) continue;
    seen.add(t);
    added.push(t);
  }
  if (added.length === 0) return { content, added: [] };
  const line = added.map((t) => `#${t}`).join(' ');
  const body = content.replace(/\s+$/, ''); // схлопываем хвостовые пробелы → чистая разделительная строка
  const next = body ? `${body}\n\n${line}\n` : `${line}\n`;
  return { content: next, added };
}

/**
 * Применяет принятые теги к заметке `path`. (1) флашит грязный открытый буфер на диск (AI-1 R1: иначе
 * writeFile затёр бы несохранённые правки тела; сбой флаша → `FlushFailedError`, не пишем); (2) читает
 * текущий контент с диска, дописывает недостающие инлайн-теги; (3) ничего не добавилось → НЕ пишем
 * (идемпотентно); (4) атомарная `write_file` (manual=снапшот истории) + анти-эхо `syncBufferAfterWrite`
 * (SAFE-3). Возвращает реально добавленные теги (пусто, если все уже были).
 */
export async function applyTags(path: string, tags: string[]): Promise<string[]> {
  await flushBufferIfDirty(path);
  const content = await tauriApi.vault.readFile(path);
  const { content: next, added } = appendInlineTags(content, tags);
  if (added.length === 0) return [];
  const hash = await tauriApi.vault.writeFile(path, next, true);
  useWorkspaceStore.getState().syncBufferAfterWrite(path, next, hash);
  return added;
}
