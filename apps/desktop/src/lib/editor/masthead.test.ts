import { describe, expect, it } from 'vitest';

import { basenameTitle, deriveMasthead, dropCapLetter } from './masthead';

describe('dropCapLetter (EDFIX-4: пунктуация-лид пропускается, символ/эмодзи-лид дисквалифицирует)', () => {
  it('берёт первую букву в верхнем регистре', () => {
    expect(dropCapLetter('много текста')).toBe('М');
    expect(dropCapLetter('the quick brown')).toBe('T');
  });
  it('пропускает ведущие пробелы/ПУНКТУАЦИЮ до первой буквы (русская проза)', () => {
    expect(dropCapLetter('  «слово»')).toBe('С');
    expect(dropCapLetter('— тире')).toBe('Т');
    expect(dropCapLetter('«Все счастливые семьи…»')).toBe('В');
    expect(dropCapLetter('— Привет, — сказал он.')).toBe('П'); // тире Pd + пробел Zs — цепочка не рвётся
    expect(dropCapLetter('„Цитата“')).toBe('Ц');
    expect(dropCapLetter('(скобка)')).toBe('С');
  });
  it('в data-cap уходит ТОЛЬКО буква (без лид-пунктуации) — CSS-зазор матчит одиночный глиф', () => {
    expect(dropCapLetter('«Вопрос»')).toBe('В'); // не '«В'
  });
  it('ведущая ЦИФРА → буквица-цифра (владелец просил «большую красную цифру»)', () => {
    expect(dropCapLetter('2026 год')).toBe('2');
    expect(dropCapLetter('123 456')).toBe('1');
    expect(dropCapLetter('  «2026»')).toBe('2'); // пропускает пунктуацию до первой цифры
    expect(dropCapLetter('— 7 правил')).toBe('7');
  });
  it('EDFIX-4 графем-гард: лид-СИМВОЛ (стрелка/эмодзи, \\p{S}) дисквалифицирует абзац → ""', () => {
    expect(dropCapLetter('← [[00 - Карта проекта]]')).toBe(''); // Рескорринг.md — репорт владельца
    expect(dropCapLetter('🚀 Запуск')).toBe('');
    expect(dropCapLetter('→ дальше по тексту')).toBe('');
  });
  it('пусто, если нет ни буквы, ни цифры', () => {
    expect(dropCapLetter('   ')).toBe('');
    expect(dropCapLetter('')).toBe('');
    expect(dropCapLetter('!!! ??? …')).toBe('');
  });
});

describe('basenameTitle', () => {
  it('срезает каталог и расширение', () => {
    expect(basenameTitle('Projects/Nexus/Идея.md')).toBe('Идея');
    expect(basenameTitle('README.markdown')).toBe('README');
    expect(basenameTitle('заметка')).toBe('заметка');
  });
  it('пусто для undefined', () => {
    expect(basenameTitle(undefined)).toBe('');
  });
});

describe('deriveMasthead — заголовок', () => {
  it('frontmatter title имеет приоритет над H1 и именем файла', () => {
    const src = '---\ntitle: Из фронтматтера\n---\n# H1 заголовок\nтекст';
    const m = deriveMasthead(src, 'file.md');
    expect(m.title).toBe('Из фронтматтера');
  });
  it('текст ведущего H1, если нет frontmatter title', () => {
    const m = deriveMasthead('# Настоящий заголовок\n\nтекст', 'file.md');
    expect(m.title).toBe('Настоящий заголовок');
    expect(m.h1Line).toBe(1);
  });
  it('имя файла, если нет ни title, ни H1', () => {
    const m = deriveMasthead('просто текст без заголовка', 'Папка/Моя заметка.md');
    expect(m.title).toBe('Моя заметка');
    expect(m.h1Line).toBeNull();
  });
  it('снимает закрывающую ATX-последовательность (# … #), но не # без пробела', () => {
    expect(deriveMasthead('# Заголовок #\nтекст', 'f.md').title).toBe('Заголовок');
    expect(deriveMasthead('# Заголовок ###\nтекст', 'f.md').title).toBe('Заголовок');
    expect(deriveMasthead('# Цена 5#\nтекст', 'f.md').title).toBe('Цена 5#'); // нет пробела → не закрытие
  });
  it('снимает inline-маркеры * и ` из отображаемого заголовка, но не из h1Text (для slug)', () => {
    const m = deriveMasthead('# Идея **важная** и `код`\nтекст', 'f.md');
    expect(m.title).toBe('Идея важная и код');
    expect(m.h1Text).toBe('Идея **важная** и `код`'); // сырой — для slugify
  });
  it('h1Text null, если ведущего H1 нет', () => {
    expect(deriveMasthead('текст без H1', 'f.md').h1Text).toBeNull();
  });
  it('срезает эмодзи из отображаемого title (daily `# 📅 …`), сырой h1Text/source целы', () => {
    const src = '# 📅 2026-03-05 Понедельник\nтекст';
    const m = deriveMasthead(src, 'f.md');
    expect(m.title).toBe('2026-03-05 Понедельник'); // title без эмодзи
    expect(m.h1Text).toBe('📅 2026-03-05 Понедельник'); // СЫРОЙ h1Text (для slug) — эмодзи цел
  });
});

describe('deriveMasthead — теги', () => {
  it('собирает теги из frontmatter, снимает ведущий #', () => {
    const m = deriveMasthead('---\ntags: [project, "#ai"]\n---\nтекст', 'f.md');
    expect(m.tags).toEqual(['project', 'ai']);
  });
  it('поддерживает блок-список тегов', () => {
    const m = deriveMasthead('---\ntags:\n  - one\n  - two\n---\nтекст', 'f.md');
    expect(m.tags).toEqual(['one', 'two']);
  });
  it('нет тегов → пустой массив', () => {
    expect(deriveMasthead('# H\nтекст', 'f.md').tags).toEqual([]);
  });
});

describe('deriveMasthead — kicker (S2: «тип · статус» из frontmatter)', () => {
  it('собирает «тип · статус» из frontmatter type/status', () => {
    const m = deriveMasthead('---\ntype: Идея\nstatus: seed\n---\nтекст', 'f.md');
    expect(m.kicker).toBe('Идея · seed');
  });
  it('только type → kicker = тип', () => {
    expect(deriveMasthead('---\ntype: Идея\n---\nтекст', 'f.md').kicker).toBe('Идея');
  });
  it('только status → kicker = статус', () => {
    expect(deriveMasthead('---\nstatus: doing\n---\nтекст', 'f.md').kicker).toBe('doing');
  });
  it('нет type/status → graceful fallback на теги', () => {
    expect(deriveMasthead('---\ntags: [project, ai]\n---\nтекст', 'f.md').kicker).toBe('project · ai');
  });
  it('type/status имеют приоритет над тегами', () => {
    const m = deriveMasthead('---\ntype: Заметка\nstatus: draft\ntags: [x, y]\n---\nтекст', 'f.md');
    expect(m.kicker).toBe('Заметка · draft');
  });
  it('нет ни type/status, ни тегов → пустой kicker', () => {
    expect(deriveMasthead('# H\nтекст', 'f.md').kicker).toBe('');
  });
  it('type/status ОСТАЮТСЯ в полях Properties (eyebrow лишь дублирует)', () => {
    const m = deriveMasthead('---\ntype: Идея\nstatus: seed\n---\nтекст', 'f.md');
    expect(m.fields.map((f) => f.key)).toEqual(['type', 'status']);
  });
});

describe('deriveMasthead — body (обнуление H1 сохраняет номера строк)', () => {
  it('обнуляет строку H1, не удаляя её — номера строк ниже не сдвигаются', () => {
    const src = '# Заголовок\n- [ ] задача';
    const m = deriveMasthead(src, 'f.md');
    const lines = m.body.split('\n');
    expect(lines.length).toBe(src.split('\n').length); // строк столько же
    expect(lines[0]).toBe(''); // H1 обнулён
    expect(lines[1]).toBe('- [ ] задача'); // задача осталась на 2-й строке (1-based 2)
  });
  it('H1 после frontmatter: обнуляется правильная строка тела', () => {
    const src = '---\nstatus: doing\n---\n# Заголовок\nтекст';
    const m = deriveMasthead(src, 'f.md');
    const lines = m.body.split('\n');
    expect(lines[3]).toBe(''); // H1 был 4-й строкой (после ---,status,---)
    expect(lines[4]).toBe('текст');
    expect(m.h1Line).toBe(4);
  });
  it('нет ведущего H1 → тело без изменений', () => {
    const src = '## Подзаголовок\nтекст';
    const m = deriveMasthead(src, 'f.md');
    expect(m.body).toBe(src);
    expect(m.h1Line).toBeNull();
  });
  it('#tag без пробела не считается H1', () => {
    const src = '#tag в начале';
    const m = deriveMasthead(src, 'f.md');
    expect(m.body).toBe(src);
    expect(m.h1Line).toBeNull();
  });
});

describe('deriveMasthead — поля для Properties (title/tags вынесены в масthead)', () => {
  it('убирает title/tags, оставляет прочие поля', () => {
    const src = '---\ntitle: T\ntags: [a]\nstatus: doing\npriority: high\n---\nтекст';
    const m = deriveMasthead(src, 'f.md');
    expect(m.fields.map((f) => f.key)).toEqual(['status', 'priority']);
  });
  it('нет frontmatter → пустые поля', () => {
    expect(deriveMasthead('# H\nтекст', 'f.md').fields).toEqual([]);
  });
});
