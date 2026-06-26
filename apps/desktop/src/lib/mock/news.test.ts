import { beforeEach, describe, expect, it, vi } from 'vitest';

// P1-8: мок `allowHost`/`disallowHost` ОБЯЗАН зеркалить бэкенд (commands/news.rs:345/365) — реально
// мутировать `extraHosts`, а не возвращать нетронутый конфиг (иначе превью/тесты «Разрешить хост»
// соврут, что хост добавлен). Свежий модуль на каждый тест — общий module-level `config` сбрасывается,
// детерминизм независимо от порядка.
describe('mock news — allowHost/disallowHost зеркалит бэкенд extra_hosts (P1-8)', () => {
  let news: typeof import('./news');
  beforeEach(async () => {
    vi.resetModules();
    news = await import('./news');
  });

  it('allowHost добавляет хост в extraHosts и возвращает применённый конфиг', async () => {
    expect((await news.getConfig()).extraHosts).toEqual([]);

    const cfg = await news.allowHost('a.com');
    expect(cfg.extraHosts).toContain('a.com');
    // Состояние действительно мутировано (getConfig видит то же).
    expect((await news.getConfig()).extraHosts).toContain('a.com');
  });

  it('allowHost идемпотентен — повтор не дублирует (как бэкенд contains-guard)', async () => {
    await news.allowHost('a.com');
    const cfg = await news.allowHost('a.com');
    expect(cfg.extraHosts.filter((h) => h === 'a.com')).toHaveLength(1);
  });

  it('disallowHost убирает хост (retain-remove); идемпотентен на отсутствующем', async () => {
    await news.allowHost('a.com');
    await news.allowHost('b.com');

    const after = await news.disallowHost('a.com');
    expect(after.extraHosts).not.toContain('a.com');
    expect(after.extraHosts).toContain('b.com');

    // Снятие отсутствующего хоста — no-op, не падает.
    const again = await news.disallowHost('a.com');
    expect(again.extraHosts).toEqual(['b.com']);
  });
});
