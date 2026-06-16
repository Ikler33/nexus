/** Мок тегов vault (DP-2) для браузерного dev/vitest — панель «Теги» сайдбара. */
import type { NoteRef, TagCount, TagSuggestion } from '../tauri-api';

const MOCK_VOCAB = ['ai', 'project', 'rag', 'planning', 'идеи'];

export async function listTags(): Promise<TagCount[]> {
  return [
    { name: 'ai', count: 12 },
    { name: 'project', count: 8 },
    { name: 'rag', count: 5 },
    { name: 'planning', count: 3 },
    { name: 'идеи', count: 2 },
  ];
}

/** AI-2c: зеркалит контракт `suggest_tags` (mock-must-match-backend). «Модель» предложила теги, мок
 *  повторяет ВСЮ логику Rust `tagger::parse_and_filter`: нормализация (trim/снять `#`/lowercase) → членство
 *  в словаре → дедуп; вне словаря → `dropped` (closed-vocab, `suggested_new` ВЫКЛ). Превью НИКОГДА не
 *  покажет тег вне словаря. Канонный вход с регистром/`#`/дублем — чтобы мок реально гонял нормализацию. */
export async function suggestTags(): Promise<TagSuggestion> {
  const modelOutput = ['#AI', 'rag', 'kubernetes', 'ai']; // '#AI'/'ai' — дубль; 'kubernetes' — вне словаря
  const vocab = new Set(MOCK_VOCAB);
  const tags: string[] = [];
  const seen = new Set<string>();
  let dropped = 0;
  for (const raw of modelOutput) {
    const t = raw.trim().replace(/^#/, '').toLowerCase();
    if (!t) continue;
    if (!vocab.has(t)) dropped += 1;
    else if (!seen.has(t)) {
      seen.add(t);
      tags.push(t);
    }
  }
  return { tags, dropped };
}

/** Заметки с ТОЧНЫМ тегом (exact-фильтр клика по тегу). Пути — реальные ключи demo-vault. */
const NOTES_BY_TAG: Record<string, NoteRef[]> = {
  ai: [
    { path: 'Notes/Idea.md', title: 'Idea' },
    { path: 'Projects/Alpha/Spec.md', title: 'Alpha Spec' },
  ],
  planning: [{ path: 'Projects/Roadmap.md', title: 'Roadmap' }],
  project: [
    { path: 'Projects/Roadmap.md', title: 'Roadmap' },
    { path: 'Projects/Alpha/Notes.md', title: 'Alpha Notes' },
  ],
};

export async function notesByTag(tag: string): Promise<NoteRef[]> {
  return NOTES_BY_TAG[tag] ?? [];
}
