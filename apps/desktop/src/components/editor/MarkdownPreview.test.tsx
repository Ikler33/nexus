import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { MarkdownPreview } from './MarkdownPreview';

// mermaid рендерится через тяжёлый dynamic-import + getBBox (jsdom не умеет) — мокаем renderMermaid
// фейковым CSP-безопасным SVG, проверяем только конвейер фенс→компонент (сам рендер — в превью).
vi.mock('../../lib/markdown/mermaid', () => ({
  renderMermaid: vi.fn(
    async () => '<svg xmlns="http://www.w3.org/2000/svg" data-mmd="1"><rect width="10" height="10"/></svg>',
  ),
}));

afterEach(() => vi.restoreAllMocks());

describe('MarkdownPreview (#20)', () => {
  it('рендерит markdown: заголовок, жирный, список', () => {
    render(<MarkdownPreview source={'# Title\n\n**bold** text\n\n- one\n- two'} onOpenLink={() => {}} />);
    expect(screen.getByRole('heading', { name: 'Title' })).toBeInTheDocument();
    expect(screen.getByText('bold')).toBeInTheDocument();
    expect(screen.getByText('one')).toBeInTheDocument();
    expect(screen.getAllByRole('listitem')).toHaveLength(2);
  });

  it('EDIT-7: заголовки несут data-outline-line (исходная строка) для перехода из оглавления', () => {
    render(<MarkdownPreview source={'# One\n\nтекст\n\n## Two'} onOpenLink={() => {}} />);
    expect(screen.getByRole('heading', { name: 'One' })).toHaveAttribute('data-outline-line', '1');
    expect(screen.getByRole('heading', { name: 'Two' })).toHaveAttribute('data-outline-line', '5');
  });

  it('GFM таблицы/таск-листы', () => {
    const src = '| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo';
    render(<MarkdownPreview source={src} onOpenLink={() => {}} />);
    expect(screen.getByRole('table')).toBeInTheDocument();
    expect(screen.getAllByRole('checkbox')).toHaveLength(2);
  });

  it('[[wikilink]] кликается → onOpenLink(target)', () => {
    const onOpen = vi.fn();
    render(<MarkdownPreview source={'go to [[My Note]] now'} onOpenLink={onOpen} />);
    const link = screen.getByText('My Note');
    fireEvent.click(link);
    expect(onOpen).toHaveBeenCalledWith('My Note');
  });

  it('#tag рендерится как чип (не ссылка)', () => {
    render(<MarkdownPreview source={'topic #ideas end'} onOpenLink={() => {}} />);
    expect(screen.getByText('#ideas')).toBeInTheDocument();
  });

  it('внутри code-fence [[x]] НЕ превращается в ссылку (mdast-уровень)', () => {
    render(<MarkdownPreview source={'```\n[[NotALink]]\n```'} onOpenLink={() => {}} />);
    // Литерал с квадратными скобками сохранён → трансформации не было.
    expect(screen.getByText(/\[\[NotALink\]\]/)).toBeInTheDocument();
  });

  it('javascript:-URL вырезается (анти-XSS, CSP-safe)', () => {
    render(<MarkdownPreview source={'[click](javascript:alert(1))'} onOpenLink={() => {}} />);
    const link = screen.getByText('click');
    expect(link.getAttribute('href') ?? '').not.toMatch(/javascript:/i);
  });

  it('EDIT-5: с onToggleTask чекбоксы кликабельны → 1-based номер исходной строки', () => {
    const onToggle = vi.fn();
    render(
      <MarkdownPreview source={'- [x] done\n- [ ] todo'} onOpenLink={() => {}} onToggleTask={onToggle} />,
    );
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes).toHaveLength(2);
    expect(boxes[0]).not.toBeDisabled(); // интерактивный, не дефолтный disabled
    fireEvent.click(boxes[1]); // второй таск — на исходной строке 2
    expect(onToggle).toHaveBeenCalledWith(2);
  });

  it('EDIT-5: без onToggleTask чекбоксы остаются read-only (disabled)', () => {
    render(<MarkdownPreview source={'- [ ] todo'} onOpenLink={() => {}} />);
    expect(screen.getByRole('checkbox')).toBeDisabled();
  });

  // Регресс на находку ревью: таск в цитате — исходная строка `> - [x]` не тогглится, значит
  // честный read-only с ВЕРНЫМ состоянием (из GFM), а не «кликабельный» чекбокс-пустышка.
  it('EDIT-5: таск в цитате — read-only с верным состоянием, клик не вызывает onToggleTask', () => {
    const onToggle = vi.fn();
    render(<MarkdownPreview source={'> - [x] quoted'} onOpenLink={() => {}} onToggleTask={onToggle} />);
    const box = screen.getByRole('checkbox');
    expect(box).toBeChecked();
    expect(box).toBeDisabled();
  });

  // Регресс на находку ревью: в loose-списке GFM кладёт input внутрь <p> — он не должен задвоиться
  // с нашим (input=>null убирает все дефолтные, в т.ч. вложенные).
  it('EDIT-5: loose-список — ровно один интерактивный чекбокс на пункт', () => {
    const onToggle = vi.fn();
    render(<MarkdownPreview source={'- [ ] a\n\n- [ ] b'} onOpenLink={() => {}} onToggleTask={onToggle} />);
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes).toHaveLength(2);
    boxes.forEach((b) => expect(b).not.toBeDisabled());
  });

  // Регресс на находку реверификации: состояние родителя не должно подменяться отмеченным дочерним
  // таском (ownTaskChecked не спускается в подсписок).
  it('EDIT-5: вложенный подсписок — состояние родителя не подменяется дочерним', () => {
    const onToggle = vi.fn();
    render(
      <MarkdownPreview source={'- [ ] parent\n  - [x] child'} onOpenLink={() => {}} onToggleTask={onToggle} />,
    );
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes).toHaveLength(2);
    expect(boxes[0]).not.toBeChecked(); // родитель НЕ отмечен
    expect(boxes[1]).toBeChecked(); // ребёнок отмечен
  });

  it('IMG-1: vault-картинка грузится как data:-URL через read_attachment', async () => {
    const read = vi
      .spyOn(tauriApi.attachments, 'read')
      .mockResolvedValue('data:image/png;base64,AAAA');
    render(<MarkdownPreview source={'![кот](attachments/cat.png)'} onOpenLink={() => {}} />);
    const img = await screen.findByRole('img', { name: 'кот' });
    expect(read).toHaveBeenCalledWith('attachments/cat.png');
    await waitFor(() => expect(img).toHaveAttribute('src', 'data:image/png;base64,AAAA'));
  });

  it('IMG-1: внешний URL картинки НЕ идёт через read_attachment', async () => {
    const read = vi.spyOn(tauriApi.attachments, 'read');
    render(<MarkdownPreview source={'![ext](https://example.com/x.png)'} onOpenLink={() => {}} />);
    const img = await screen.findByRole('img', { name: 'ext' });
    expect(img).toHaveAttribute('src', 'https://example.com/x.png');
    expect(read).not.toHaveBeenCalled();
  });
});

describe('MarkdownPreview: формулы (#4, MathML под строгим CSP)', () => {
  it('инлайн $$…$$ рендерится в <math>', () => {
    const { container } = render(<MarkdownPreview source={'энергия $$E=mc^2$$ тут'} onOpenLink={() => {}} />);
    const math = container.querySelector('math');
    expect(math).not.toBeNull();
    expect(math?.querySelector('msup')).not.toBeNull(); // mc^2 → надстрочный
  });

  it('блочная $$…$$ рендерится в <math>', () => {
    const { container } = render(
      <MarkdownPreview source={'$$\\int_0^1 x\\,dx$$'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).not.toBeNull();
  });

  // Регресс: $$ внутри инлайн-кода НЕ парсится как математика (remark-math уважает приоритет code).
  it('$$ внутри инлайн-кода остаётся литералом, не математикой', () => {
    const { container } = render(<MarkdownPreview source={'код `$$x$$` тут'} onOpenLink={() => {}} />);
    expect(container.querySelector('math')).toBeNull();
    expect(screen.getByText('$$x$$')).toBeInTheDocument();
  });

  it('$$ внутри code-fence не превращается в формулу', () => {
    const { container } = render(
      <MarkdownPreview source={'```\n$$a+b$$\n```'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).toBeNull();
  });

  // Сосуществование: math + wikilink + tag на одной строке — remarkNexus не сломан remark-math.
  // Тег ASCII (`#idea`): бэкенд создаёт только ASCII-теги, превью теперь совпадает (кириллица — текст).
  it('формула уживается с [[wikilink]] и #tag', () => {
    const { container } = render(
      <MarkdownPreview source={'$$x$$ и [[Заметка]] и #idea'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).not.toBeNull();
    expect(screen.getByText('Заметка')).toBeInTheDocument();
    expect(screen.getByText('#idea')).toBeInTheDocument();
  });

  // БАГ adversarial-ревью реализации #5: двойной-$ валюты в заметках о деньгах НЕ должен становиться
  // математикой (политика singleDollarTextMath:false — одиночный $ = валюта, не формула).
  it('суммы с $ (валюта) не парсятся как формула', () => {
    const { container } = render(
      <MarkdownPreview source={'зарплата с $5000 до $7000 и ещё $200'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).toBeNull();
    expect(screen.getByText(/5000/)).toBeInTheDocument();
  });

  // Блокер adversarial-ревью дизайна: фиксируем output:'mathml' — на валидной формуле НЕТ инлайн-стилей
  // (регресс к default htmlAndMathml вернул бы сотни style="" на span → CSP-violation, тихо в jsdom).
  it('CSP-guard: валидная формула не содержит ни одного инлайн-style', () => {
    const { container } = render(<MarkdownPreview source={'$$a^2+b^2=c^2$$'} onOpenLink={() => {}} />);
    expect(container.querySelector('math')).not.toBeNull();
    expect(container.querySelectorAll('[style]')).toHaveLength(0);
  });

  // Блокер adversarial-ревью дизайна (error-path): битый LaTeX не роняет рендер И не оставляет инлайн-
  // style на .katex-error (rehypeKatexCsp снял его; цвет — через CSS-класс). Иначе CSP-violation в вебвью.
  it('битый LaTeX: фолбэк без краша и без инлайн-style (CSP-safe)', () => {
    const { container } = render(<MarkdownPreview source={'сломано $$\\frac{1}{$$'} onOpenLink={() => {}} />);
    expect(container.querySelectorAll('[style]')).toHaveLength(0);
  });

  // БАГ adversarial-ревью реализации #2: \fcolorbox даёт <mpadded style="border:…"> ДАЖЕ под output:
  // 'mathml' — стриппер обязан снять и его (не только .katex-error), иначе CSP-violation на валидной формуле.
  it('\\fcolorbox: рамочный стиль снят (CSP-safe, не только error-узлы)', () => {
    const { container } = render(
      <MarkdownPreview source={'$$\\fcolorbox{red}{yellow}{x}$$'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).not.toBeNull();
    expect(container.querySelectorAll('[style]')).toHaveLength(0);
  });
});

describe('MarkdownPreview: транклюзия ![[embed]] (Live-Preview, режим чтения)', () => {
  it('блок-вставка ![[Note]] рекурсивно рендерит тело + заголовок-ссылку', async () => {
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Target.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('---\nstatus: x\n---\n# Hello\n\nWorld body');
    const onOpen = vi.fn();
    render(<MarkdownPreview source={'![[Target]]'} onOpenLink={onOpen} />);

    // тело вставки появляется асинхронно (резолв + чтение)
    expect(await screen.findByText('World body')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Hello' })).toBeInTheDocument();
    // frontmatter срезан — `status: x` не отрисован
    expect(screen.queryByText(/status: x/)).not.toBeInTheDocument();
    // заголовок-ссылка ведёт к исходной заметке (как клик по вики-ссылке)
    fireEvent.click(screen.getByText('Target'));
    expect(onOpen).toHaveBeenCalledWith('Target');
  });

  it('![[Note#Heading]] вставляет только секцию заголовка', async () => {
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/T.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue(
      '# A\n\nalpha\n\n## Section\n\ninside body\n\n## Other\n\noutside body',
    );
    render(<MarkdownPreview source={'![[T#Section]]'} onOpenLink={() => {}} />);

    expect(await screen.findByText('inside body')).toBeInTheDocument();
    expect(screen.queryByText('outside body')).not.toBeInTheDocument();
    expect(screen.queryByText('alpha')).not.toBeInTheDocument();
  });

  it('несуществующая заметка → заглушка «не найдена» с целью', async () => {
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue(null);
    const read = vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('x');
    render(<MarkdownPreview source={'![[Ghost]]'} onOpenLink={() => {}} />);

    expect(await screen.findByText(/Ghost/)).toBeInTheDocument();
    expect(read).not.toHaveBeenCalled();
  });

  it('гард-цикл: ![[self]] в своей же заметке → «циклическая», файл НЕ читается', async () => {
    const resolve = vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Self.md');
    const read = vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('# Self\n\nloop');
    render(<MarkdownPreview source={'![[Self]]'} notePath={'Notes/Self.md'} onOpenLink={() => {}} />);

    await waitFor(() => expect(resolve).toHaveBeenCalled());
    expect(await screen.findByText(/[Цц]иклическая/)).toBeInTheDocument();
    expect(read).not.toHaveBeenCalled();
  });

  it('инлайн ![[X]] (не отдельный абзац) НЕ становится вставкой → старое поведение', async () => {
    const resolve = vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/X.md');
    render(<MarkdownPreview source={'текст ![[X]] дальше'} onOpenLink={() => {}} />);

    // вики-ссылка X есть (remarkNexus), а вставку не резолвили
    expect(screen.getByText('X')).toBeInTheDocument();
    await Promise.resolve();
    expect(resolve).not.toHaveBeenCalled();
  });

  it('![[pic.png]] (картинка) НЕ резолвится как заметка (вне охвата слайса)', async () => {
    const resolve = vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/pic.md');
    render(<MarkdownPreview source={'![[pic.png]]'} onOpenLink={() => {}} />);
    await Promise.resolve();
    expect(resolve).not.toHaveBeenCalled();
  });

  // Ревью транклюзии (нит fan-out): потолок 50 вставок на заметку — 55 абзацев → резолвится ровно 50.
  it('потолок вставок на заметку: 55 ![[…]] → не более 50 резолвов (остальные — fallback)', async () => {
    const resolve = vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/x.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('# x\n\ny');
    const src = Array.from({ length: 55 }, (_, i) => `![[N${i}]]`).join('\n\n');
    render(<MarkdownPreview source={src} onOpenLink={() => {}} />);
    await waitFor(() => expect(resolve).toHaveBeenCalledTimes(50));
  });
});

describe('MarkdownPreview: картинки-вставки ![[pic.png]] (IMG-EMBED)', () => {
  it('![[diagram.png]] → резолв basename + рендер <img> с data-URL', async () => {
    vi.spyOn(tauriApi.attachments, 'resolve').mockResolvedValue('Notes/diagram.png');
    const read = vi
      .spyOn(tauriApi.attachments, 'read')
      .mockResolvedValue('data:image/png;base64,AAAA');
    render(<MarkdownPreview source={'![[diagram.png]]'} onOpenLink={() => {}} />);

    const img = await screen.findByRole('img');
    expect(tauriApi.attachments.resolve).toHaveBeenCalledWith('diagram.png');
    await waitFor(() => expect(read).toHaveBeenCalledWith('Notes/diagram.png'));
    await waitFor(() => expect(img).toHaveAttribute('src', 'data:image/png;base64,AAAA'));
  });

  it('![[pic.png|подпись|250]] → alt и width(атрибут, CSP-safe)', async () => {
    vi.spyOn(tauriApi.attachments, 'resolve').mockResolvedValue('attachments/pic.png');
    vi.spyOn(tauriApi.attachments, 'read').mockResolvedValue('data:image/png;base64,BBBB');
    render(<MarkdownPreview source={'![[pic.png|подпись|250]]'} onOpenLink={() => {}} />);

    const img = await screen.findByRole('img', { name: 'подпись' });
    expect(img).toHaveAttribute('width', '250');
    // ширина — HTML-атрибут, не inline-style (строгий CSP)
    expect(img.getAttribute('style')).toBeNull();
  });

  it('картинка не найдена → честная заглушка, не битый <img>', async () => {
    vi.spyOn(tauriApi.attachments, 'resolve').mockResolvedValue(null);
    const read = vi.spyOn(tauriApi.attachments, 'read');
    render(<MarkdownPreview source={'![[ghost.png]]'} onOpenLink={() => {}} />);

    expect(await screen.findByText(/ghost\.png/)).toBeInTheDocument();
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    expect(read).not.toHaveBeenCalled();
  });

  it('![[pic.png]] идёт по image-пути, не через транклюзию заметок', async () => {
    vi.spyOn(tauriApi.attachments, 'resolve').mockResolvedValue('attachments/pic.png');
    vi.spyOn(tauriApi.attachments, 'read').mockResolvedValue('data:image/png;base64,CCCC');
    const resolveNote = vi.spyOn(tauriApi.vault, 'resolveNote');
    render(<MarkdownPreview source={'![[pic.png]]'} onOpenLink={() => {}} />);
    await screen.findByRole('img');
    expect(resolveNote).not.toHaveBeenCalled();
  });
});

describe('MarkdownPreview: Mermaid-диаграммы (```mermaid)', () => {
  it('```mermaid фенс → рендерит SVG (а не код-блок)', async () => {
    const { container } = render(
      <MarkdownPreview source={'```mermaid\ngraph TD; A-->B;\n```'} onOpenLink={() => {}} />,
    );
    await waitFor(() => expect(container.querySelector('svg[data-mmd]')).not.toBeNull());
    expect(container.querySelector('code')).toBeNull(); // не остался обычным код-блоком
  });

  it('обычный ```js фенс остаётся код-блоком (mermaid не трогает чужие языки)', () => {
    const { container } = render(
      <MarkdownPreview source={'```js\nconst x = 1;\n```'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('code')).not.toBeNull();
    expect(container.querySelector('svg[data-mmd]')).toBeNull();
  });

  it('callout > [!warning]: data-callout, иконка-svg, заголовок и тело', () => {
    const { container } = render(
      <MarkdownPreview source={'> [!warning] Осторожно\n> тело предупреждения'} onOpenLink={() => {}} />,
    );
    const callout = container.querySelector('[data-callout]');
    expect(callout).not.toBeNull();
    expect(callout?.getAttribute('data-callout')).toBe('warning');
    expect(callout?.querySelector('svg')).not.toBeNull(); // lucide-иконка (инлайновый SVG, CSP-safe)
    expect(screen.getByText('Осторожно')).toBeInTheDocument();
    expect(screen.getByText('тело предупреждения')).toBeInTheDocument();
    expect(container.querySelector('blockquote')).toBeNull(); // не осталась обычной цитатой
  });

  it('callout-алиас > [!error] нормализуется в canonical danger', () => {
    const { container } = render(<MarkdownPreview source={'> [!error] Сбой'} onOpenLink={() => {}} />);
    expect(container.querySelector('[data-callout]')?.getAttribute('data-callout')).toBe('danger');
  });

  it('callout без заголовка → дефолтная подпись по типу', () => {
    render(<MarkdownPreview source={'> [!note]\n> текст'} onOpenLink={() => {}} />);
    expect(screen.getByText('Note')).toBeInTheDocument();
  });

  it('сворачиваемый callout [!info]-: тело скрыто, клик по шапке раскрывает', () => {
    render(<MarkdownPreview source={'> [!info]- Детали\n> скрытое тело'} onOpenLink={() => {}} />);
    expect(screen.queryByText('скрытое тело')).not.toBeInTheDocument(); // свёрнут по умолчанию
    fireEvent.click(screen.getByRole('button', { name: /Детали/ }));
    expect(screen.getByText('скрытое тело')).toBeInTheDocument();
  });

  it('обычная цитата (без маркера) остаётся blockquote', () => {
    const { container } = render(<MarkdownPreview source={'> просто цитата'} onOpenLink={() => {}} />);
    expect(container.querySelector('blockquote')).not.toBeNull();
    expect(container.querySelector('[data-callout]')).toBeNull();
  });

  it('[[wikilink]] внутри тела callout кликается', () => {
    const onOpen = vi.fn();
    render(<MarkdownPreview source={'> [!tip] Совет\n> см. [[Другая]]'} onOpenLink={onOpen} />);
    fireEvent.click(screen.getByText('Другая'));
    expect(onOpen).toHaveBeenCalledWith('Другая');
  });

  it('==выделение== рендерится <mark>', () => {
    const { container } = render(<MarkdownPreview source={'обычный ==жёлтый== текст'} onOpenLink={() => {}} />);
    const mark = container.querySelector('mark');
    expect(mark).not.toBeNull();
    expect(mark?.textContent).toBe('жёлтый');
  });

  it('== не ломает ~~strike~~ и **bold** (отдельные узлы)', () => {
    const { container } = render(
      <MarkdownPreview source={'~~зачёрк~~ **жирн** ==марк=='} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('del')).not.toBeNull();
    expect(container.querySelector('strong')).not.toBeNull();
    expect(container.querySelector('mark')?.textContent).toBe('марк');
  });

  it('== внутри code-fence НЕ становится <mark> (mdast-уровень)', () => {
    const { container } = render(<MarkdownPreview source={'```\nx ==y== z\n```'} onOpenLink={() => {}} />);
    expect(container.querySelector('mark')).toBeNull();
    expect(screen.getByText(/==y==/)).toBeInTheDocument();
  });
});
