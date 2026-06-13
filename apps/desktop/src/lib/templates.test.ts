import { beforeEach, describe, expect, it } from 'vitest';
import { useVaultStore } from '../stores/vault';
import { activeBuffer, useWorkspaceStore } from '../stores/workspace';
import { applyPlaceholders, listTemplates, newNoteFromTemplate, templateTitle } from './templates';

const NOW = new Date(2026, 5, 13, 9, 5); // 2026-06-13 09:05 (локально)

beforeEach(async () => {
  useWorkspaceStore.getState().reset();
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  await useVaultStore.getState().openVault('');
});

describe('templates (CAP-3)', () => {
  it('templateTitle: basename без .md', () => {
    expect(templateTitle('Templates/Meeting.md')).toBe('Meeting');
    expect(templateTitle('Daily.md')).toBe('Daily');
  });

  it('applyPlaceholders: подставляет {{date}} {{time}} {{datetime}} {{title}}', () => {
    const out = applyPlaceholders(
      '{{title}} — {{date}} {{time}} ({{ datetime }})',
      'Встреча',
      NOW,
    );
    expect(out).toBe('Встреча — 2026-06-13 09:05 (2026-06-13 09:05)');
    expect(out).not.toMatch(/\{\{/); // не осталось плейсхолдеров
  });

  it('applyPlaceholders: title с $-спецсимволами подставляется буквально', () => {
    // String.replace со строкой трактует $&/$1 как спецпаттерны — должны идти буквально.
    expect(applyPlaceholders('# {{title}}', 'Счёт $50 ($&)', NOW)).toBe('# Счёт $50 ($&)');
  });

  it('listTemplates: только .md из Templates/', async () => {
    const list = await listTemplates();
    expect(list).toContain('Templates/Meeting.md');
    expect(list).toContain('Templates/Daily.md');
    expect(list.every((p) => p.endsWith('.md'))).toBe(true);
  });

  it('newNoteFromTemplate: создаёт заметку с подстановкой и открывает её', async () => {
    const path = await newNoteFromTemplate('Templates/Meeting.md', NOW);
    expect(path).toBe('Meeting.md'); // имя из шаблона, в корне (нет активной заметки)
    const buf = activeBuffer(useWorkspaceStore.getState());
    expect(buf?.path).toBe('Meeting.md');
    expect(buf?.doc).toContain('# Meeting'); // {{title}} → Meeting
    expect(buf?.doc).toContain('2026-06-13 09:05'); // {{date}} {{time}}
    expect(buf?.doc).not.toMatch(/\{\{/);
  });
});
