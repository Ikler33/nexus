import { describe, expect, it } from 'vitest';

import { argDetail, describeStep } from './describeStep';

/** Фейковый `t`: кодирует ключ (+ интерполяцию) в строку — детерминированная проверка маппинга. */
const t = (key: string, opts?: Record<string, unknown>): string =>
  opts && 'q' in opts ? `${key}::${String(opts.q)}` : key;

const step = (kind: string, args = '{}', title?: string | null) => ({ kind, args, title });

describe('describeStep', () => {
  it('подпись от агента (title) выигрывает над шаблоном', () => {
    expect(describeStep(step('fetch', '{"url":"docs.rs"}', 'Fetching docs.rs'), t)).toBe(
      'Fetching docs.rs',
    );
  });

  it('пустой/whitespace title игнорируется → шаблон', () => {
    expect(describeStep(step('fetch', '{"url":"docs.rs"}', '   '), t)).toBe('agent.act.fetch::docs.rs');
    expect(describeStep(step('fetch', '{"url":"docs.rs"}', null), t)).toBe('agent.act.fetch::docs.rs');
  });

  it('длинный title усекается', () => {
    const long = 'x'.repeat(200);
    const out = describeStep(step('fetch', '{}', long), t);
    expect(out.length).toBeLessThanOrEqual(120);
    expect(out.endsWith('…')).toBe(true);
  });

  it('web.search + query → ключ search с деталью', () => {
    expect(describeStep(step('web.search', '{"query":"rust async"}'), t)).toBe(
      'agent.act.search::rust async',
    );
  });

  it('Hermes-ACP короткий kind маппится так же, как Castor-точечный', () => {
    expect(describeStep(step('search', '{"query":"q"}'), t)).toBe('agent.act.search::q');
    expect(describeStep(step('edit', '{"path":"A.md"}'), t)).toBe('agent.act.edit::A.md');
    expect(describeStep(step('fetch', '{"url":"u"}'), t)).toBe('agent.act.fetch::u');
  });

  it('file-глаголы показывают path', () => {
    expect(describeStep(step('note.create', '{"path":"Notes/X.md"}'), t)).toBe(
      'agent.act.create::Notes/X.md',
    );
    expect(describeStep(step('note.edit', '{"path":"Y.md"}'), t)).toBe('agent.act.edit::Y.md');
  });

  it('command-глаголы показывают command', () => {
    expect(describeStep(step('shell', '{"command":"cargo build"}'), t)).toBe(
      'agent.act.command::cargo build',
    );
    expect(describeStep(step('execute', '{"command":"ls"}'), t)).toBe('agent.act.command::ls');
  });

  it('think/plan — без аргумента (bare-ключ)', () => {
    expect(describeStep(step('think', '{}'), t)).toBe('agent.act.think');
    expect(describeStep(step('plan', '{"anything":"ignored"}'), t)).toBe('agent.act.plan');
  });

  it('известный kind, но деталь не распарсилась → bare-ключ', () => {
    expect(describeStep(step('fetch', 'не json'), t)).toBe('agent.act.fetchBare');
    expect(describeStep(step('note.edit', '{"other":"no path"}'), t)).toBe('agent.act.editBare');
  });

  it('неизвестный/будущий kind → сырой kind (не падаем)', () => {
    expect(describeStep(step('some.future.tool', '{}'), t)).toBe('some.future.tool');
  });

  it('длинная деталь усекается до ≤64', () => {
    const out = describeStep(step('fetch', JSON.stringify({ url: 'u'.repeat(200) })), t);
    // 'agent.act.fetch::' + усечённый url
    const detail = out.split('::')[1];
    expect(detail.length).toBeLessThanOrEqual(64);
    expect(detail.endsWith('…')).toBe(true);
  });
});

describe('argDetail', () => {
  it('берёт предпочитаемый ключ', () => {
    expect(argDetail('{"path":"p","url":"u"}', 'url')).toBe('u');
    expect(argDetail('{"path":"p","url":"u"}', 'path')).toBe('p');
  });

  it('фолбэк на path|query|command|url по порядку', () => {
    expect(argDetail('{"query":"q"}')).toBe('q');
    expect(argDetail('{"command":"c"}')).toBe('c');
  });

  it('кривой JSON → undefined', () => {
    expect(argDetail('{ broken')).toBeUndefined();
  });

  it('нет строковых значений → undefined', () => {
    expect(argDetail('{"path":123,"url":null}')).toBeUndefined();
    expect(argDetail('{}')).toBeUndefined();
  });
});
