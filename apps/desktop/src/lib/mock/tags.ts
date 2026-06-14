/** Мок тегов vault (DP-2) для браузерного dev/vitest — панель «Теги» сайдбара. */
import type { NoteRef, TagCount } from '../tauri-api';

export async function listTags(): Promise<TagCount[]> {
  return [
    { name: 'ai', count: 12 },
    { name: 'project', count: 8 },
    { name: 'rag', count: 5 },
    { name: 'planning', count: 3 },
    { name: 'идеи', count: 2 },
  ];
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
