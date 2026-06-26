import { describe, expect, it } from 'vitest';

import {
  bodyExcerpt,
  footnoteNumber,
  footnoteText,
  noteStatus,
  noteTitle,
  noteType,
} from './popcard';

describe('popcard helpers (Hermes-8 S7)', () => {
  describe('noteType / noteStatus (frontmatter, анти-фейк: только реальные поля)', () => {
    it('читает type/status из frontmatter', () => {
      const c = '---\ntype: idea\nstatus: seed\n---\n\nтело';
      expect(noteType(c)).toBe('idea');
      expect(noteStatus(c)).toBe('seed');
    });
    it('нет поля → null (не выдумываем)', () => {
      expect(noteType('просто тело без frontmatter')).toBeNull();
      expect(noteStatus('---\ntitle: X\n---\nтело')).toBeNull();
    });
  });

  describe('noteTitle (frontmatter title → H1 → basename)', () => {
    it('берёт frontmatter title', () => {
      expect(noteTitle('---\ntitle: Моя идея\n---\nтело', 'notes/a.md')).toBe('Моя идея');
    });
    it('без title — первый H1 тела', () => {
      expect(noteTitle('# Заголовок\n\nтело', 'notes/a.md')).toBe('Заголовок');
    });
    it('без title и H1 — basename пути (честный фолбэк, не выдуманное имя)', () => {
      expect(noteTitle('просто тело', 'Notes/My Note.md')).toBe('My Note');
    });
  });

  describe('bodyExcerpt (эксцерпт тела, без frontmatter/H1, обрезка по слову)', () => {
    it('сглаживает markdown и убирает frontmatter + ведущий H1', () => {
      const c = '---\ntype: idea\n---\n# Заголовок\n\nЭто **тело** с [[ссылкой]] и `code`.';
      const ex = bodyExcerpt(c);
      expect(ex).toContain('Это тело с ссылкой и code');
      expect(ex).not.toContain('Заголовок');
      expect(ex).not.toContain('type:');
    });
    it('обрезает по слову + «…» при превышении лимита', () => {
      const long = 'слово '.repeat(80).trim();
      const ex = bodyExcerpt(long, 50);
      expect(ex.endsWith('…')).toBe(true);
      expect(ex.length).toBeLessThanOrEqual(52);
      expect(ex).not.toMatch(/слов$/); // не режет посреди слова
    });
    it('короткое тело — без «…»', () => {
      expect(bodyExcerpt('коротко')).toBe('коротко');
    });
    it('закрытый код-фенс вырезается целиком', () => {
      const ex = bodyExcerpt('перед\n\n```js\nconst x = 1;\n```\n\nпосле');
      expect(ex).not.toContain('const x');
      expect(ex).toContain('перед');
      expect(ex).toContain('после');
    });
    // FIX 2 (MAJOR): НЕзакрытый фенс не должен утечь сырым телом (секреты) в превью.
    it('НЕзакрытый код-фенс не утекает (секрет НЕ в эксцерпте)', () => {
      const ex = bodyExcerpt('видимый текст\n\n```yaml\napi_key: sk-SECRET123\ntoken: t-LEAK\n');
      expect(ex).toContain('видимый текст');
      expect(ex).not.toContain('api_key');
      expect(ex).not.toContain('sk-SECRET123');
      expect(ex).not.toContain('token');
      expect(ex).not.toContain('LEAK');
    });
  });

  describe('footnoteNumber (N из href/id)', () => {
    it('извлекает N из #user-content-fn-N', () => {
      expect(footnoteNumber('#user-content-fn-3')).toBe('3');
      expect(footnoteNumber('user-content-fn-note1')).toBe('note1');
    });
    it('не-сноска → null', () => {
      expect(footnoteNumber('#heading')).toBeNull();
    });
  });

  describe('footnoteText (textContent <li> минус backref ↩)', () => {
    it('срезает backref-якорь и нормализует пробелы', () => {
      const li = document.createElement('li');
      li.innerHTML = 'текст сноски <a href="#x" class="data-footnote-backref">↩</a>';
      expect(footnoteText(li)).toBe('текст сноски');
    });
    it('null → пусто', () => {
      expect(footnoteText(null)).toBe('');
    });
    // FIX 5(a) (MINOR): длинный текст сноски обрезается по слову + «…» (карточка не переполняет вьюпорт).
    it('длинный текст обрезается по слову + «…»', () => {
      const li = document.createElement('li');
      li.textContent = 'длинно '.repeat(120).trim(); // > 400 симв
      const out = footnoteText(li, 100);
      expect(out.endsWith('…')).toBe(true);
      expect(out.length).toBeLessThanOrEqual(102);
      expect(out).not.toMatch(/длин$/); // не режет посреди слова
    });
    it('короткий текст — без обрезки', () => {
      const li = document.createElement('li');
      li.textContent = 'коротко';
      expect(footnoteText(li)).toBe('коротко');
    });
  });
});
