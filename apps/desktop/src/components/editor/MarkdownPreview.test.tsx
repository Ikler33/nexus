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

describe('MarkdownPreview: S3 сворачиваемые H2-секции', () => {
  it('группирует h2+контент в <section.sec data-sec-id>; лид до 1-го h2 — вне секций', () => {
    const { container } = render(
      <MarkdownPreview source={'интро-лид\n\n## Раздел\n\nтело секции'} onOpenLink={() => {}} />,
    );
    const sec = container.querySelector('section[data-sec-id="раздел"]');
    expect(sec).not.toBeNull();
    // h2 — первый ребёнок секции, тело внутри секции
    expect(sec?.querySelector('h2')).not.toBeNull();
    expect(sec?.textContent).toContain('тело секции');
    // лид «интро-лид» — НЕ внутри секции (вне неё)
    const lead = screen.getByText('интро-лид');
    expect(lead.closest('section[data-sec-id]')).toBeNull();
  });

  it('HEADANCHOR-1: h2 в секции ОСТАЁТСЯ heading и СОХРАНЯЕТ id(slug) + data-outline-line', () => {
    render(<MarkdownPreview source={'# One\n\nтекст\n\n## Раздел Два'} onOpenLink={() => {}} />);
    // h2 — по-прежнему heading (role не перебит на button → Outline/scroll-spy/getByRole целы)
    const h2 = screen.getByRole('heading', { name: /Раздел Два/, level: 2 });
    expect(h2).toHaveAttribute('id', 'раздел-два');
    expect(h2).toHaveAttribute('data-outline-line', '5'); // исходная строка h2
  });

  it('клик по h2 сворачивает секцию (класс collapsed + aria-expanded шеврона); повторный — разворачивает', () => {
    const { container } = render(
      <MarkdownPreview source={'## Заголовок\n\nтело'} onOpenLink={() => {}} />,
    );
    // re-query: react-markdown пересоздаёт DOM-узел секции на каждый ре-рендер (старая ссылка протухает)
    const sec = () => container.querySelector('section[data-sec-id]') as HTMLElement;
    const h2 = () => screen.getByRole('heading', { name: /Заголовок/, level: 2 });
    // развёрнуто по умолчанию
    expect(sec().className).not.toMatch(/collapsed/);
    expect(screen.getByRole('button', { name: /секцию/ })).toHaveAttribute('aria-expanded', 'true');
    fireEvent.click(h2()); // клик по строке заголовка тогглит
    expect(sec().className).toMatch(/collapsed/);
    expect(screen.getByRole('button', { name: /секцию/ })).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(h2());
    expect(sec().className).not.toMatch(/collapsed/);
  });

  it('шеврон-кнопка тогглит секцию без двойного срабатывания (stopPropagation)', () => {
    const { container } = render(<MarkdownPreview source={'## H\n\nтело'} onOpenLink={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: /секцию/ }));
    // один тоггл → свёрнуто (не «свернул→развернул» из-за всплытия на h2)
    expect((container.querySelector('section[data-sec-id]') as HTMLElement).className).toMatch(/collapsed/);
  });

  it('тоггл одной секции не сворачивает другую (ключ — стабильный data-sec-id)', () => {
    const { container } = render(
      <MarkdownPreview source={'## Альфа\n\na\n\n## Бета\n\nb'} onOpenLink={() => {}} />,
    );
    fireEvent.click(screen.getByRole('heading', { name: /Альфа/, level: 2 }));
    const secA = container.querySelector('section[data-sec-id="альфа"]') as HTMLElement;
    const secB = container.querySelector('section[data-sec-id="бета"]') as HTMLElement;
    expect(secA.className).toMatch(/collapsed/);
    expect(secB.className).not.toMatch(/collapsed/); // соседняя не тронута
  });

  it('тело секции НЕ размонтируется при сворачивании (раскрытие из оглавления/scroll-spy)', () => {
    render(<MarkdownPreview source={'## H\n\nскрываемое тело'} onOpenLink={() => {}} />);
    fireEvent.click(screen.getByRole('heading', { name: /H/, level: 2 }));
    // CSS прячет (max-height:0), но узел в DOM остаётся (jsdom не считает max-height)
    expect(screen.getByText('скрываемое тело')).toBeInTheDocument();
  });

  it('документ без H2 не падает и не создаёт секций (плоский рендер)', () => {
    const { container } = render(
      <MarkdownPreview source={'# Только H1\n\nпросто абзац\n\n- список'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('section[data-sec-id]')).toBeNull();
    expect(screen.getByText('просто абзац')).toBeInTheDocument();
    expect(screen.getByText('список')).toBeInTheDocument();
  });

  it('EDIT-5: таск внутри секции остаётся кликабельным с верной исходной строкой', () => {
    const onToggle = vi.fn();
    // строки: 1 `## Дела`, 2 пусто, 3 `- [ ] купить`
    render(
      <MarkdownPreview
        source={'## Дела\n\n- [ ] купить'}
        onOpenLink={() => {}}
        onToggleTask={onToggle}
      />,
    );
    const box = screen.getByRole('checkbox');
    // чекбокс ВНУТРИ секции
    expect(box.closest('section[data-sec-id]')).not.toBeNull();
    expect(box).not.toBeDisabled();
    fireEvent.click(box);
    expect(onToggle).toHaveBeenCalledWith(3);
  });

  it('FOOTNOTE-1: блок сносок (`<section.footnotes>`) сохраняет класс (не наша секция → passthrough)', () => {
    const { container } = render(
      <MarkdownPreview source={'текст[^1]\n\n[^1]: определение'} onOpenLink={() => {}} />,
    );
    // GFM-секция сносок проходит через override section, но сохраняет .footnotes-класс
    expect(container.querySelector('.footnotes, [class*=footnotes]')).not.toBeNull();
    expect(screen.getByText(/определение/)).toBeInTheDocument();
  });

  it('секции уживаются с masthead: ведущий H1 → шапка, H2 → секции', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# Заголовок\n\nлид\n\n## Раздел\n\nтело'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    // шапка есть
    expect(container.querySelector('[class*=docHead]')).not.toBeNull();
    // H2 — секция; лид «лид» вне секции (до первого H2)
    expect(container.querySelector('section[data-sec-id="раздел"]')).not.toBeNull();
    expect(screen.getByText('лид').closest('section[data-sec-id]')).toBeNull();
  });

  // Регресс ревью (MINOR-2): сноски не должны прятаться при сворачивании ПОСЛЕДНЕЙ секции (GFM-блок
  // footnotes выносится top-level, а не в тело секции).
  it('FOOTNOTE-1: сноски ВИДНЫ при свёрнутой последней секции (footnotes вне секции)', () => {
    const { container } = render(
      <MarkdownPreview source={'## Раздел\n\nтекст[^1]\n\n[^1]: определение'} onOpenLink={() => {}} />,
    );
    // блок сносок — НЕ внутри секции
    const fnBlock = container.querySelector('.footnotes, [class*=footnotes]');
    expect(fnBlock).not.toBeNull();
    expect(fnBlock?.closest('section[data-sec-id]')).toBeNull();
    // свернём секцию — определение сноски остаётся в DOM и вне свёрнутого тела
    fireEvent.click(screen.getByRole('heading', { name: /Раздел/, level: 2 }));
    const fnAfter = container.querySelector('.footnotes, [class*=footnotes]');
    expect(fnAfter?.closest('section[data-sec-id]')).toBeNull();
    expect(screen.getByText(/определение/)).toBeInTheDocument();
  });

  // Регресс ревью (MINOR-3): смена ЗАМЕТКИ (notePath) сбрасывает свёрнутость (нет утечки stale secId).
  it('смена notePath сбрасывает свёрнутость секций (нет утечки stale secId)', () => {
    const { container, rerender } = render(
      <MarkdownPreview source={'## Раздел\n\nтело'} notePath="a.md" onOpenLink={() => {}} />,
    );
    fireEvent.click(screen.getByRole('heading', { name: /Раздел/, level: 2 }));
    expect((container.querySelector('section[data-sec-id="раздел"]') as HTMLElement).className).toMatch(
      /collapsed/,
    );
    // другая заметка с СЕКЦИЕЙ ТОГО ЖЕ slug — должна открыться РАЗВЁРНУТОЙ (стейт сброшен по notePath)
    rerender(<MarkdownPreview source={'## Раздел\n\nиное тело'} notePath="b.md" onOpenLink={() => {}} />);
    expect((container.querySelector('section[data-sec-id="раздел"]') as HTMLElement).className).not.toMatch(
      /collapsed/,
    );
  });

  // Анти-регресс к MINOR-3-фиксу: правка source ТОЙ ЖЕ заметки (notePath не сменился) свёрнутость СОХРАНЯЕТ
  // (иначе сворачивание терялось бы на каждое нажатие клавиши).
  it('правка source без смены notePath НЕ сбрасывает свёрнутость', () => {
    const { container, rerender } = render(
      <MarkdownPreview source={'## Раздел\n\nтело'} notePath="a.md" onOpenLink={() => {}} />,
    );
    fireEvent.click(screen.getByRole('heading', { name: /Раздел/, level: 2 }));
    rerender(<MarkdownPreview source={'## Раздел\n\nтело правлено'} notePath="a.md" onOpenLink={() => {}} />);
    expect((container.querySelector('section[data-sec-id="раздел"]') as HTMLElement).className).toMatch(
      /collapsed/,
    );
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

  it('TAGCLICK-1: #tag-чип кликабелен при onOpenTag → вызывает с именем тега (без #)', () => {
    const onOpenTag = vi.fn();
    render(<MarkdownPreview source={'topic #ideas end'} onOpenLink={() => {}} onOpenTag={onOpenTag} />);
    const chip = screen.getByRole('button', { name: '#ideas' });
    fireEvent.click(chip);
    expect(onOpenTag).toHaveBeenCalledWith('ideas');
  });

  it('TAGCLICK-1: #tag-чип кликается с клавиатуры (Enter)', () => {
    const onOpenTag = vi.fn();
    render(<MarkdownPreview source={'#project'} onOpenLink={() => {}} onOpenTag={onOpenTag} />);
    fireEvent.keyDown(screen.getByRole('button', { name: '#project' }), { key: 'Enter' });
    expect(onOpenTag).toHaveBeenCalledWith('project');
  });

  it('TAGCLICK-1: тег нормализуется в нижний регистр (бэкенд хранит lowercase)', () => {
    const onOpenTag = vi.fn();
    render(<MarkdownPreview source={'#TODO note'} onOpenLink={() => {}} onOpenTag={onOpenTag} />);
    fireEvent.click(screen.getByRole('button', { name: '#TODO' })); // показывается как написано
    expect(onOpenTag).toHaveBeenCalledWith('todo'); // но фильтр — в нижнем регистре
  });

  it('TAGCLICK-1: без onOpenTag чип НЕ кликабелен (не button, честно)', () => {
    render(<MarkdownPreview source={'topic #ideas end'} onOpenLink={() => {}} />);
    expect(screen.queryByRole('button', { name: '#ideas' })).toBeNull();
    expect(screen.getByText('#ideas')).toBeInTheDocument();
  });

  it('COMMENT-1: %%коммент%% скрыт в режиме чтения', () => {
    render(<MarkdownPreview source={'видно %%скрыто%% дальше'} onOpenLink={() => {}} />);
    expect(screen.getByText(/видно/)).toBeInTheDocument();
    expect(screen.queryByText(/скрыто/)).toBeNull();
    expect(screen.queryByText(/%%/)).toBeNull();
  });

  it('COMMENT-1: %% внутри code-fence НЕ вырезается', () => {
    const { container } = render(
      <MarkdownPreview source={'```\nx %%y%% z\n```'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('code')?.textContent).toContain('%%y%%');
  });

  it('HEADANCHOR-1: заголовок получает slug-id, дубликаты дедуплицируются', () => {
    render(<MarkdownPreview source={'# Hello World\n\n## Intro\n\n## Intro'} onOpenLink={() => {}} />);
    expect(screen.getByRole('heading', { name: 'Hello World' })).toHaveAttribute('id', 'hello-world');
    const intros = screen.getAllByRole('heading', { name: 'Intro' });
    expect(intros[0]).toHaveAttribute('id', 'intro');
    expect(intros[1]).toHaveAttribute('id', 'intro-1');
  });

  it('FOOTNOTE-1: сноска [^1] рендерит ссылку-надстрочник и блок .footnotes', () => {
    const { container } = render(
      <MarkdownPreview source={'текст[^1]\n\n[^1]: определение сноски'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('sup')).not.toBeNull(); // ссылка-надстрочник
    expect(container.querySelector('.footnotes, [class*=footnotes], section')).not.toBeNull();
    expect(screen.getByText(/определение сноски/)).toBeInTheDocument();
  });

  it('FOOTNOTE-1: клик по ссылке-сноске скроллит к её определению (CSS.escape + scope)', () => {
    // jsdom не реализует scrollIntoView → подменяем заглушкой (configurable, чтобы restore не падал).
    const scrollSpy = vi.fn();
    Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', {
      value: scrollSpy,
      configurable: true,
      writable: true,
    });
    const { container } = render(
      <MarkdownPreview source={'текст[^1]\n\n[^1]: определение'} onOpenLink={() => {}} />,
    );
    const ref = container.querySelector('sup a');
    expect(ref).not.toBeNull();
    fireEvent.click(ref as Element);
    expect(scrollSpy).toHaveBeenCalled();
  });

  it('хеш-ссылка с литеральным % (#50%) не роняет клик (гард decodeURIComponent)', () => {
    render(<MarkdownPreview source={'[go](#50%)'} onOpenLink={() => {}} />);
    // Клик не должен бросать URIError (битое %-кодирование ловится try/catch).
    expect(() => fireEvent.click(screen.getByText('go'))).not.toThrow();
  });

  it('FRONTMATTER-1: frontmatter → Properties-таблица, сырой YAML и лишний hr не показываются', () => {
    const { container } = render(
      <MarkdownPreview source={'---\ntitle: Моя\nstatus: doing\n---\n\n# Тело'} onOpenLink={() => {}} />,
    );
    expect(screen.getByText('title')).toBeInTheDocument();
    expect(screen.getByText('Моя')).toBeInTheDocument();
    expect(screen.getByText('status')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Тело' })).toBeInTheDocument();
    expect(container.querySelector('hr')).toBeNull(); // нет спурьёзного thematicBreak от ---
    expect(screen.queryByText(/^---$/)).toBeNull();
  });

  it('FRONTMATTER-1: строки тела НЕ сдвигаются — таск после frontmatter тоглится по верной строке', () => {
    const onToggleTask = vi.fn();
    // Строки: 1 `---`, 2 `title: X`, 3 `---`, 4 пусто, 5 `- [ ] дело`.
    render(
      <MarkdownPreview
        source={'---\ntitle: X\n---\n\n- [ ] дело'}
        onOpenLink={() => {}}
        onToggleTask={onToggleTask}
      />,
    );
    fireEvent.click(screen.getByRole('checkbox'));
    expect(onToggleTask).toHaveBeenCalledWith(5); // 1-based строка ПОЛНОГО исходника (не срезанного)
  });

  it('FRONTMATTER-1: поле tags — кликабельные чипы через onOpenTag (lowercase)', () => {
    const onOpenTag = vi.fn();
    render(
      <MarkdownPreview source={'---\ntags: [Work, Idea]\n---\n\nтекст'} onOpenLink={() => {}} onOpenTag={onOpenTag} />,
    );
    fireEvent.click(screen.getByRole('button', { name: '#Work' }));
    expect(onOpenTag).toHaveBeenCalledWith('work');
  });

  it('FRONTMATTER-1: без frontmatter таблицы нет', () => {
    const { container } = render(<MarkdownPreview source={'# Просто заголовок'} onOpenLink={() => {}} />);
    expect(container.querySelector('[class*=properties]')).toBeNull();
  });
});

describe('MarkdownPreview: MASTHEAD-1 (editorial-шапка + буквица)', () => {
  it('рисует kicker · display-title · byline; ведущий H1 не дублируется', () => {
    const src = '---\ntags: [project, ai]\n---\n# Главный заголовок\n\nМного текста для буквицы.';
    render(
      <MarkdownPreview source={src} notePath="Notes/Test.md" onOpenLink={() => {}} masthead={{ mtime: null }} />,
    );
    // kicker — нет type/status → graceful fallback на теги через « · »
    expect(screen.getByText('project · ai')).toBeInTheDocument();
    // заголовок — РОВНО один (H1 тела погашен, не задвоен)
    const titles = screen.getAllByRole('heading', { name: 'Главный заголовок' });
    expect(titles).toHaveLength(1);
    // outline → H1 по-прежнему ведёт к шапке (строка исходного H1)
    expect(titles[0]).toHaveAttribute('data-outline-line', '4');
    // byline — слова и время чтения
    expect(screen.getByText(/мин чтения/)).toBeInTheDocument();
    // тело отрисовано
    expect(screen.getByText('Много текста для буквицы.')).toBeInTheDocument();
  });

  it('заголовок из frontmatter title имеет приоритет; title/tags не дублируются в Properties', () => {
    const src = '---\ntitle: Заголовок ФМ\ntags: [a]\nstatus: doing\n---\n# H1 тела\n\nтекст';
    const { container } = render(
      <MarkdownPreview source={src} notePath="f.md" onOpenLink={() => {}} masthead={{ mtime: null }} />,
    );
    expect(screen.getByRole('heading', { name: 'Заголовок ФМ' })).toBeInTheDocument();
    // S2: kicker — «тип · статус»; здесь только status → eyebrow = «doing» (значение есть и в Properties,
    // потому ищем строго в самом eyebrow'е).
    expect(container.querySelector('[class*=docKicker]')?.textContent).toBe('doing');
    expect(screen.getByText('status')).toBeInTheDocument(); // ключ status — в Properties (не вынесен)
    expect(screen.queryByText('title')).toBeNull(); // ключ title не дублируется
    expect(screen.queryByText('tags')).toBeNull(); // ключ tags не дублируется
  });

  it('S2: eyebrow = «тип · статус» из frontmatter; type/status остаются в Properties', () => {
    const { container } = render(
      <MarkdownPreview
        source={'---\ntype: Идея\nstatus: seed\ntags: [x]\n---\n# Заголовок\n\nтекст'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    // eyebrow — type · status (теги в приоритете ниже)
    expect(container.querySelector('[class*=docKicker]')?.textContent).toBe('Идея · seed');
    // оба ключа по-прежнему видны в Properties-таблице
    expect(screen.getByText('type')).toBeInTheDocument();
    expect(screen.getByText('status')).toBeInTheDocument();
  });

  it('буквица: первая буква ведущего абзаца штампуется в data-cap', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# Зачин\n\nМного текста для буквицы здесь.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.getAttribute('data-cap')).toBe('М');
  });

  it('буквица — ТОЛЬКО на абзаце-зачине: список первым блоком → буквицы нет', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# T\n\n- пункт один\n- пункт два'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    expect(container.querySelector('[data-dropcap]')).toBeNull();
  });

  it('без masthead: нет ни шапки, ни буквицы (embed/peek/доска не меняются)', () => {
    const { container } = render(
      <MarkdownPreview source={'# Заголовок\n\nМного текста.'} notePath="f.md" onOpenLink={() => {}} />,
    );
    expect(container.querySelector('[class*=docHead]')).toBeNull();
    expect(container.querySelector('[data-dropcap]')).toBeNull();
    // H1 тела рендерится как обычный markdown-заголовок (тело не тронуто)
    expect(screen.getByRole('heading', { name: 'Заголовок' })).toBeInTheDocument();
  });

  it('HEADANCHOR-1: slug ведущего H1 переносится на заголовок шапки; дедуп одноимённых не сдвигается', () => {
    // Ведущий H1 «Обзор» погашен → его slug 'обзор' уходит на заголовок шапки (и потребляется первым,
    // поэтому второй «Обзор» в теле получает 'обзор-1', как было бы без шапки).
    render(
      <MarkdownPreview
        source={'# Обзор\n\nтекст\n\n## Обзор'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const title = screen.getByRole('heading', { name: 'Обзор', level: 1 });
    expect(title).toHaveAttribute('id', 'обзор');
    expect(screen.getByRole('heading', { name: 'Обзор', level: 2 })).toHaveAttribute('id', 'обзор-1');
  });

  it('строки тела не сдвигаются: таск под погашенным H1 тоглится по верной строке', () => {
    const onToggleTask = vi.fn();
    // Строки: 1 `# H`, 2 пусто, 3 `- [ ] дело`.
    render(
      <MarkdownPreview
        source={'# H\n\n- [ ] дело'}
        notePath="f.md"
        onOpenLink={() => {}}
        onToggleTask={onToggleTask}
        masthead={{ mtime: null }}
      />,
    );
    fireEvent.click(screen.getByRole('checkbox'));
    expect(onToggleTask).toHaveBeenCalledWith(3); // строка ПОЛНОГО исходника, не сдвинута погашением H1
  });
});
