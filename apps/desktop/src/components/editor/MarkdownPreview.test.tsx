import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createRef } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { MarkdownPreview, type MarkdownPreviewHandle } from './MarkdownPreview';

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

describe('MarkdownPreview: S6 revealLine (раскрытие секции при прыжке оглавления)', () => {
  // Источник: h2 «Раздел» — строка 1; вложенный h3 «Под» — строка 3; тело — строка 5.
  const SRC = '## Раздел\n\n### Под\n\nтекст внутри';

  it('imperative-хэндл доходит через forwardRef (revealLine — функция на ref.current)', () => {
    const ref = createRef<MarkdownPreviewHandle>();
    render(<MarkdownPreview ref={ref} source={SRC} onOpenLink={() => {}} />);
    expect(ref.current).not.toBeNull();
    expect(typeof ref.current?.revealLine).toBe('function');
  });

  it('revealLine к строке внутри свёрнутой секции раскрывает её + возвращает true (didExpand)', () => {
    const ref = createRef<MarkdownPreviewHandle>();
    const { container } = render(<MarkdownPreview ref={ref} source={SRC} onOpenLink={() => {}} />);
    // Свернём секцию кликом по h2.
    fireEvent.click(screen.getByRole('heading', { name: /Раздел/, level: 2 }));
    const sec = () => container.querySelector('section[data-sec-id="раздел"]') as HTMLElement;
    expect(sec().className).toMatch(/collapsed/);
    // Прыжок к вложенному h3 (строка 3, скрыт в свёрнутом теле) → revealLine раскрывает + сигналит true.
    let didExpand: boolean | undefined;
    act(() => {
      didExpand = ref.current?.revealLine(3);
    });
    expect(didExpand).toBe(true); // S6-FIX2: реально раскрыл → GroupPane отложит scrollIntoView
    expect(sec().className).not.toMatch(/collapsed/);
    // Тело смонтировано и видимо (узел в DOM — он и не размонтировался по S3).
    expect(screen.getByText('текст внутри')).toBeInTheDocument();
  });

  it('revealLine идемпотентна: уже развёрнутая секция → false (no-op, скролл будет немедленным)', () => {
    const ref = createRef<MarkdownPreviewHandle>();
    const { container } = render(<MarkdownPreview ref={ref} source={SRC} onOpenLink={() => {}} />);
    const sec = () => container.querySelector('section[data-sec-id="раздел"]') as HTMLElement;
    expect(sec().className).not.toMatch(/collapsed/);
    let didExpand: boolean | undefined;
    act(() => {
      didExpand = ref.current?.revealLine(3); // уже развёрнута
    });
    expect(didExpand).toBe(false); // S6-FIX2: ничего не раскрывал → немедленный скролл
    expect(sec().className).not.toMatch(/collapsed/);
  });

  it('revealLine к строке вне секций (лид/нет такой строки) → false, безопасный no-op', () => {
    const ref = createRef<MarkdownPreviewHandle>();
    const { container } = render(
      <MarkdownPreview ref={ref} source={'лид-интро\n\n## Раздел\n\nтело'} onOpenLink={() => {}} />,
    );
    // Свернём секцию.
    fireEvent.click(screen.getByRole('heading', { name: /Раздел/, level: 2 }));
    const sec = () => container.querySelector('section[data-sec-id="раздел"]') as HTMLElement;
    expect(sec().className).toMatch(/collapsed/);
    // Строки без data-outline-line (лид / несуществующая) → секция не трогается, didExpand=false.
    let r1: boolean | undefined;
    let r2: boolean | undefined;
    act(() => {
      r1 = ref.current?.revealLine(1); // лид — вне секции
      r2 = ref.current?.revealLine(999); // нет такой строки
    });
    expect(r1).toBe(false);
    expect(r2).toBe(false);
    expect(sec().className).toMatch(/collapsed/); // осталась свёрнутой (целевой секции не было)
  });

  // S6-FIX1: смена «лёгкого» стейта (сворачивание секции) НЕ должна ре-парсить тело — `<ReactMarkdown>`
  // мемоизирован по [body, components], collapsedSecs идёт через SectionContext СНАРУЖИ мемо-границы.
  // Признак отсутствия ре-парса: DOM-узел абзаца тела сохраняет ИДЕНТИЧНОСТЬ через toggle (React переиспользовал
  // мемо-элемент, а не пересоздал поддерево из нового hast). При этом класс .collapsed корректно проставляется.
  it('S6-FIX1: toggle секции применяет .collapsed, но НЕ пересоздаёт поддерево тела (идентичность узла цела)', () => {
    const { container } = render(
      <MarkdownPreview source={'## Раздел\n\nабзац тела'} onOpenLink={() => {}} />,
    );
    const bodyP = screen.getByText('абзац тела'); // конкретный DOM-узел абзаца ДО сворачивания
    const sec = () => container.querySelector('section[data-sec-id="раздел"]') as HTMLElement;
    expect(sec().className).not.toMatch(/collapsed/);
    fireEvent.click(screen.getByRole('heading', { name: /Раздел/, level: 2 })); // toggle через контекст
    // S3 жив: класс .collapsed проставился (контекст пробил мемо-границу к Section).
    expect(sec().className).toMatch(/collapsed/);
    // FIX1: тот же DOM-узел абзаца (тело НЕ ре-парсилось — иначе был бы новый элемент).
    expect(screen.getByText('абзац тела')).toBe(bodyP);
  });
});

describe('MarkdownPreview: S5 блочные элементы «Редакция»', () => {
  it('hr (`---`) → астеризм: div[role=separator] с тремя точками-span, без <hr>', () => {
    const { container } = render(
      <MarkdownPreview source={'текст до\n\n---\n\nтекст после'} onOpenLink={() => {}} />,
    );
    const sep = container.querySelector('[role="separator"]');
    expect(sep).not.toBeNull();
    expect(sep?.tagName).toBe('DIV'); // не <hr> — декоративный астеризм с семантикой separator
    expect(sep?.querySelectorAll('span')).toHaveLength(3); // три точки
    expect(container.querySelector('hr')).toBeNull(); // голого <hr> не осталось
  });

  it('blockquote без accent-фоновой плашки (нет inline background); левый бордер цитаты остаётся', () => {
    const { container } = render(<MarkdownPreview source={'> цитата мудреца'} onOpenLink={() => {}} />);
    const bq = container.querySelector('blockquote');
    expect(bq).not.toBeNull();
    // фон/радиус-плашка убраны — стиль раздаёт CSS через `.preview blockquote` (element-селектор, не
    // class на узле), inline-background не задаётся. jsdom не вычисляет module-CSS, проверяем что нет
    // inline-style (accent-плашка убрана в CSS, не в разметке).
    expect(bq?.getAttribute('style')).toBeNull();
    expect(screen.getByText('цитата мудреца')).toBeInTheDocument();
  });

  it('table: thead в <thead>, tbody в <tbody> (mono-разделитель/serif — через CSS-module)', () => {
    const src = '| Имя | Возраст |\n|---|---|\n| Алиса | 30 |\n| Боб | 25 |';
    const { container } = render(<MarkdownPreview source={src} onOpenLink={() => {}} />);
    const table = container.querySelector('table');
    expect(table).not.toBeNull();
    expect(table?.querySelector('thead th')).not.toBeNull(); // заголовок колонки в thead → mono-стиль
    expect(table?.querySelectorAll('tbody tr')).toHaveLength(2);
    expect(screen.getByText('Алиса')).toBeInTheDocument();
  });

  it('callout [!info] → slate-вид (data-callout=info, иконка, тело)', () => {
    const { container } = render(
      <MarkdownPreview source={'> [!info] Заметка\n> важная деталь'} onOpenLink={() => {}} />,
    );
    const callout = container.querySelector('[data-callout="info"]');
    expect(callout).not.toBeNull(); // info-вид (slate-палитра через per-kind CSS)
    expect(callout?.querySelector('svg')).not.toBeNull();
    expect(screen.getByText('важная деталь')).toBeInTheDocument();
  });

  it('код-блок (```...```) рендерит <pre><code> (mono/left-border — через CSS, без хайлайт-токенов)', () => {
    const { container } = render(
      <MarkdownPreview source={'```\nconst x = 1;\n```'} onOpenLink={() => {}} />,
    );
    const pre = container.querySelector('pre');
    expect(pre).not.toBeNull();
    expect(pre?.querySelector('code')).not.toBeNull();
    // нет синтаксического хайлайтера в превью → нет токен-span .cm/.kw/.st (базовый pre)
    expect(pre?.querySelectorAll('span')).toHaveLength(0);
  });

  it('footnotes-блок: сохраняет .footnotes-класс и якоря сносок (passthrough S3 не сломан)', () => {
    const { container } = render(
      <MarkdownPreview source={'текст[^1]\n\n[^1]: определение сноски'} onOpenLink={() => {}} />,
    );
    const fn = container.querySelector('.footnotes, [class*=footnotes]');
    expect(fn).not.toBeNull();
    // якорь-id сноски цел (FOOTNOTE-1) — нумерация-рескин его не трогает
    expect(container.querySelector('li[id*="fn-1"]')).not.toBeNull();
    expect(screen.getByText(/определение сноски/)).toBeInTheDocument();
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

  // ── БАГ 1: эмодзи срезаны из заголовков (H1 masthead-title + H2-секции), source НЕ мутируется ──
  it('эмодзи срезаны из masthead-title и H2-секции; исходный source не меняется', () => {
    const src = '# 📅 2026-03-05 Понедельник\n\n## 🧠 Поток мыслей\n\nТело секции с буквицей.';
    const { container } = render(
      <MarkdownPreview source={src} notePath="Daily/2026-03-05.md" onOpenLink={() => {}} masthead={{ mtime: null }} />,
    );
    // masthead-title — без эмодзи (id docTitle начинается с цифры → читаем через класс, не getByRole)
    expect(container.querySelector('[class*=docTitle]')?.textContent).toBe('2026-03-05 Понедельник');
    // H2-секция — без эмодзи (SectionHeading рендерит h2)
    expect(screen.getByRole('heading', { name: 'Поток мыслей', level: 2 })).toBeInTheDocument();
    // секция получила slug БЕЗ эмодзи
    expect(container.querySelector('section[data-sec-id="поток-мыслей"]')).not.toBeNull();
    // эмодзи в заголовках в DOM нет
    expect(container.textContent).not.toContain('🧠');
    expect(container.textContent).not.toContain('📅');
    // КРИТИЧНО: исходная строка (пропс source) не мутирована — эмодзи остаются в .md
    expect(src).toBe('# 📅 2026-03-05 Понедельник\n\n## 🧠 Поток мыслей\n\nТело секции с буквицей.');
  });

  it('эмодзи срезаны из H3–H6 тела (не только секционных H2)', () => {
    const { container } = render(
      <MarkdownPreview
        source={'## Раздел\n\n### 💡 Подзаголовок\n\nтекст'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    expect(screen.getByRole('heading', { name: 'Подзаголовок', level: 3 })).toBeInTheDocument();
    expect(container.textContent).not.toContain('💡');
  });

  it('эмодзи в ТЕЛЕ абзаца НЕ трогаются (strip только заголовки)', () => {
    render(
      <MarkdownPreview
        source={'## Заголовок\n\nТекст абзаца с эмодзи 🎉 целым.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    expect(screen.getByText('Текст абзаца с эмодзи 🎉 целым.')).toBeInTheDocument();
  });

  // ── БАГ 2a: буквица появляется, когда заметка открывается H2-СЕКЦИЕЙ (первый абзац вложен в section) ──
  it('буквица: заметка открывается H2-секцией → первый <p> внутри секции получает data-dropcap', () => {
    const { container } = render(
      <MarkdownPreview
        source={'## 🧠 Поток мыслей\n\nМного текста для буквицы внутри секции.'}
        notePath="Daily/2026-03-05.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.getAttribute('data-cap')).toBe('М');
    // абзац-цель реально лежит ВНУТРИ секции (S3-обёртка)
    expect(cap?.closest('section[data-sec-id]')).not.toBeNull();
  });

  // ── EDFIX-4 КОРЕНЬ 1 (race, репорт владельца 2026-07-02): React РЕМОУНТИТ DOM-дерево превью при
  //    смене identity `components`-useMemo (новый колбэк-проп из его deps, напр. inline-стрелка
  //    `onOpenLink={(t) => …}` в GroupPane на КАЖДЫЙ ре-рендер, в т.ч. асинхронный setMtime) →
  //    узел-носитель data-dropcap уничтожается. Эффект штамповки обязан перештамповать ПОСЛЕ ремоунта.
  //    До фикса deps эффекта были [body, mastheadActive] — ремоунт невидим → буквица терялась
  //    (живая репродукция: 1/6 циклов открытия preview успешен). ──
  it('EDFIX-4 race: буквица переживает ремоунт превью (смена identity колбэк-пропа)', () => {
    const src = '## X\n\nАбзац';
    const { container, rerender } = render(
      <MarkdownPreview source={src} notePath="f.md" onOpenLink={() => {}} masthead={{ mtime: null }} />,
    );
    // После первого коммита буквица есть (первый «голый» абзац внутри H2-секции).
    expect(container.querySelector('p[data-dropcap]')).not.toBeNull();
    // Новая identity onOpenLink (тот же контракт, что живой GroupPane) → `components` инвалидируется →
    // markdownEl пересоздаётся → section-поддерево ремоунтится, <p> пересоздан БЕЗ атрибута.
    rerender(
      <MarkdownPreview source={src} notePath="f.md" onOpenLink={() => {}} masthead={{ mtime: null }} />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.getAttribute('data-cap')).toBe('А');
  });

  // ── EDFIX-4 КОРЕНЬ 2 (графем-гард): буквица только на абзаце, НАЧИНАЮЩЕМСЯ с буквы/цифры.
  //    Реальный кейс владельца (Рескорринг.md): лид-абзац `← [[00 - Карта проекта]]` получал
  //    data-cap='0' (первая цифра ГДЕ УГОДНО в тексте), а CSS ::first-letter раздувал «←». ──
  it('EDFIX-4 графем-гард: абзац с лидом «←» ПРОПУСКАЕТСЯ — буквица на следующем обычном абзаце', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# T\n\n← [[00 - Карта проекта]]\n\nНормальный лид-абзац заметки.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const caps = container.querySelectorAll('p[data-dropcap]');
    expect(caps).toHaveLength(1);
    expect(caps[0].textContent).toContain('Нормальный лид-абзац');
    expect(caps[0].getAttribute('data-cap')).toBe('Н');
    // Абзац со стрелкой — БЕЗ атрибутов буквицы.
    const arrow = Array.from(container.querySelectorAll('p')).find((p) =>
      (p.textContent ?? '').startsWith('←'),
    );
    expect(arrow).toBeTruthy();
    expect(arrow?.hasAttribute('data-dropcap')).toBe(false);
  });

  it('EDFIX-4 графем-гард: единственный абзац с лидом «←» → буквицы нет вовсе', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# T\n\n← [[00 - Карта проекта]]'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    expect(container.querySelector('[data-dropcap]')).toBeNull();
  });

  it('EDFIX-4 графем-гард: абзац с лидом-цифрой `2026 год…` → data-cap=«2» (цифра-буквица не регрессирует)', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# T\n\n2026 год начался с больших планов.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.getAttribute('data-cap')).toBe('2');
  });

  // ── РЕГРЕССИЯ (репорт владельца, реальная daily-заметка Daily/2026-03-01.md): режим чтения должен
  //    давать ЧИСТЫЕ вики-ссылки (без `[[ ]]`) + буквицу. Воспроизводит ВЕСЬ реальный конвейер на
  //    БАЙТ-ТОЧНОМ содержимом (frontmatter type/status, эмодзи-H1/H2, H2 `##  Связи` — после `##`
  //    идёт ASCII-пробел + NO-BREAK SPACE U+00A0, как в .md владельца, НЕ два ASCII-пробела; список с
  //    **жирным**, ПУСТАЯ вики `[[ ]]`, датированная `[[2026-02-24 - 2026-03-02]]`, callout `> [!tip]-`).
  //    Прежние юнит-тесты гоняли синтетический контент и НЕ ловили бы регрессию пайплайна
  //    (remarkStripHeadingEmoji → remarkNexus) на этих edge-узлах. Подтверждено идентично в реальном
  //    WebKit (WKWebView, движок Tauri): рендер не падает, ссылки чисты, буквица есть. ──
  it('РЕГРЕССИЯ реальной daily-заметки (reading): рендер без краха, чистые ссылки, буквица', () => {
    const REAL = [
      '---',
      'type: daily',
      'status: processed',
      '---',
      '',
      '',
      '',
      '# 📅 воскресенье, март 1-го 2026',
      '',
      '## 🧠 Поток мыслей',
      '',
      'Сегодня день рождения - бестолковый день, так как я ничего хорошего не сделал.',
      '',
      'Аргументы против покупки ноутбука:',
      '',
      '- У меня стационарный, мощный пк, которые потянет **любые** задачи',
      '- Я не выхожу из дома.',
      '',
      '## 💡 Кандидаты в идеи',
      '- [[ ]]',
      '##  Связи', // байт-точно: `##` + ASCII-пробел + NBSP (U+00A0), как в реальном .md владельца
      '',
      '- [[2026-02-24 - 2026-03-02]]',
      '> [!tip]- 🧩 Шпаргалка (статусы daily)',
      '> **inbox** → новая заметка',
    ].join('\n');
    // Рендер НЕ должен бросать (нет ErrorBoundary вокруг превью → краш = белый экран в живом app).
    const { container } = render(
      <MarkdownPreview
        source={REAL}
        notePath="Daily/2026-03-01.md"
        masthead={{ mtime: Date.now(), reading: true }}
        onOpenLink={() => {}}
        onOpenTag={() => {}}
        onToggleTask={() => {}}
      />,
    );
    // 1) Вики-ссылки преобразованы в чистые `<a>` — сырых `[[ ]]` в DOM НЕТ (regression #1 владельца).
    expect(container.textContent).not.toContain('[[');
    expect(container.textContent).not.toContain(']]');
    // Датированная ссылка — кликабельный wikilink с чистым лейблом (без скобок).
    const dated = Array.from(container.querySelectorAll('a[class*="wikilink"]')).find(
      (a) => a.textContent === '2026-02-24 - 2026-03-02',
    );
    expect(dated).toBeTruthy();
    expect(dated?.getAttribute('href')).toBe('#'); // wikilink-навигация через onOpenLink, не href
    // Пустая `[[ ]]` не роняет пайплайн и тоже не оставляет скобок (рендерится как пустой/минимальный <a>).
    expect(container.querySelectorAll('a[class*="wikilink"]').length).toBeGreaterThanOrEqual(2);
    // 2) Буквица ведущего абзаца проставлена (regression #2 владельца): первый «обычный» абзац — внутри
    //    H2-секции `## 🧠 Поток мыслей`, начинается с «Сегодня» → data-cap='С'.
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.getAttribute('data-cap')).toBe('С');
    // 3) Эмодзи срезаны из заголовков, masthead отрисован, double-space H2 `##  Связи` → секция «Связи».
    expect(container.textContent).not.toContain('📅');
    expect(container.textContent).not.toContain('🧠');
    expect(container.querySelector('[class*="docHead"]')).not.toBeNull();
    expect(container.querySelector('section[data-sec-id="связи"]')).not.toBeNull();
  });

  it('буквица в секции — только ПЕРВЫЙ абзац первой секции (не во второй секции)', () => {
    const { container } = render(
      <MarkdownPreview
        source={'## Первый\n\nАбзац первой секции.\n\n## Второй\n\nАбзац второй секции.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const caps = container.querySelectorAll('p[data-dropcap]');
    expect(caps).toHaveLength(1); // ровно одна буквица — в первой секции
    expect(caps[0].textContent).toContain('Абзац первой секции.');
  });

  it('буквица в секции — если первый блок секции список (не абзац), буквицы нет', () => {
    const { container } = render(
      <MarkdownPreview
        source={'## Раздел\n\n- пункт один\n- пункт два'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    expect(container.querySelector('[data-dropcap]')).toBeNull();
  });

  // ── Регресс БАГ 2: счётчик секций (`.sec h2::before`) появляется для всех top-level H2 ──
  it('регресс: счётчик секций цел — каждый top-level H2 обёрнут в section[data-sec-id]', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# Заголовок\n\nлид\n\n## Раздел A\n\nтекст\n\n## Раздел B\n\nтекст'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    // обе секции обёрнуты (CSS-счётчик counter(sec) тикает по `.sec` → 01/02 появятся)
    const secs = container.querySelectorAll('section[data-sec-id]');
    expect(secs).toHaveLength(2);
    expect(container.querySelector('section[data-sec-id="раздел-a"]')).not.toBeNull();
    expect(container.querySelector('section[data-sec-id="раздел-b"]')).not.toBeNull();
  });

  // adversarial FIX 1 (CRITICAL): заголовок с inline-марками НЕ склеивает слова при рендере.
  it('FIX1: H2 с **bold** не склеивает слова (`Раздел **A** и B` → «Раздел A и B»)', () => {
    render(
      <MarkdownPreview
        source={'## Раздел **A** и B\n\nтекст'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    // accessible name заголовка собирается из всех inline-узлов с пробелами на стыках
    expect(screen.getByRole('heading', { name: 'Раздел A и B', level: 2 })).toBeInTheDocument();
  });

  // adversarial FIX 3 (MAJOR): буквица НЕ садится внутрь callout/blockquote, если секция ими открывается.
  it('FIX3: секция открывается callout → буквица НЕ внутри callout (на лид-абзаце ниже)', () => {
    const { container } = render(
      <MarkdownPreview
        source={'## Раздел\n\n> [!note] Заметка\n> тело callout\n\nНастоящий лид-абзац секции.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    // буквица НЕ внутри admonition
    expect(cap?.closest('[data-callout]')).toBeNull();
    // буквица на реальном лид-абзаце
    expect(cap?.textContent).toContain('Настоящий лид-абзац секции.');
  });

  it('FIX3: секция открывается blockquote → буквица НЕ внутри цитаты (на абзаце ниже)', () => {
    const { container } = render(
      <MarkdownPreview
        source={'## Раздел\n\n> цитата в начале\n\nЛид-абзац после цитаты.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.closest('blockquote')).toBeNull();
    expect(cap?.textContent).toContain('Лид-абзац после цитаты.');
  });

  it('FIX3: секция = ТОЛЬКО callout (нет голого абзаца) → буквицы нет', () => {
    const { container } = render(
      <MarkdownPreview
        source={'## Раздел\n\n> [!note] Заметка\n> только тело callout'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    expect(container.querySelector('[data-dropcap]')).toBeNull();
  });

  // ── Обобщение: буквица на ПЕРВОМ обычном абзаце тела в порядке чтения (а не только первом блоке) ──
  it('обобщение: callout первым блоком тела → буквица на следующем обычном абзаце (не в callout)', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# Зачин\n\n> [!note] Преамбула\n> тело callout\n\nНастоящий лид-абзац документа.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.closest('[data-callout]')).toBeNull();
    expect(cap?.textContent).toContain('Настоящий лид-абзац документа.');
  });

  it('обобщение: blockquote первым блоком тела → буквица на следующем обычном абзаце (не в цитате)', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# Зачин\n\n> вступительная цитата\n\nЛид-абзац документа после цитаты.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.closest('blockquote')).toBeNull();
    expect(cap?.textContent).toContain('Лид-абзац документа после цитаты.');
  });

  it('обобщение: список первым блоком тела → буквица на первом абзаце-вне-списка', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# Зачин\n\n- пункт один\n- пункт два\n\nОбычный абзац после списка.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.closest('li')).toBeNull();
    expect(cap?.textContent).toContain('Обычный абзац после списка.');
  });

  it('обобщение: первый обычный абзац ниже по документу, не в первой секции, всё равно ловится', () => {
    // Первая секция = только список (нет голого абзаца) → буквица уходит на абзац ВТОРОЙ секции.
    const { container } = render(
      <MarkdownPreview
        source={'## Первый\n\n- только пункт\n\n## Второй\n\nПервый обычный абзац всего документа.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const caps = container.querySelectorAll('p[data-dropcap]');
    expect(caps).toHaveLength(1);
    expect(caps[0].textContent).toContain('Первый обычный абзац всего документа.');
    expect(caps[0].closest('section[data-sec-id="второй"]')).not.toBeNull();
  });

  it('обобщение: абзац начинается с ЦИФРЫ → буквица-цифра', () => {
    const { container } = render(
      <MarkdownPreview
        source={'# Зачин\n\n2026 год был особенным во многих смыслах.'}
        notePath="f.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    const cap = container.querySelector('p[data-dropcap]');
    expect(cap).not.toBeNull();
    expect(cap?.getAttribute('data-cap')).toBe('2');
  });

  it('скоуп эмбеда: <p> вложенного ![[embed]] НЕ получает буквицу (буквица только в своём документе)', async () => {
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Target.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('# Hello\n\nWorld body of embed.');
    const { container } = render(
      <MarkdownPreview
        // Документ открывается ВСТАВКОЙ: единственный «обычный» абзац — внутри вложенной .preview эмбеда.
        source={'![[Target]]'}
        notePath="Notes/Host.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    // дожидаемся асинхронного рендера тела эмбеда
    expect(await screen.findByText('World body of embed.')).toBeInTheDocument();
    // вложенный абзац эмбеда не получил буквицы (внешний эффект скоупнут на свой previewRef)
    expect(container.querySelector('p[data-dropcap]')).toBeNull();
  });

  it('скоуп эмбеда: свой лид-абзац получает буквицу, абзац эмбеда НИЖЕ — нет', async () => {
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Target.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('# Hello\n\nWorld body of embed.');
    const { container } = render(
      <MarkdownPreview
        source={'# Зачин\n\nСвой лид-абзац документа-хоста.\n\n![[Target]]'}
        notePath="Notes/Host.md"
        onOpenLink={() => {}}
        masthead={{ mtime: null }}
      />,
    );
    expect(await screen.findByText('World body of embed.')).toBeInTheDocument();
    const caps = container.querySelectorAll('p[data-dropcap]');
    expect(caps).toHaveLength(1);
    expect(caps[0].textContent).toContain('Свой лид-абзац документа-хоста.');
    // абзац эмбеда без буквицы
    expect(screen.getByText('World body of embed.').closest('p')?.hasAttribute('data-dropcap')).toBe(false);
  });
});

// Hermes-8 S7 — ховер-превью `.popcard`. Fake timers (220мс вики / 120мс сноска) + мок резолвера/readFile.
// jsdom: getBoundingClientRect → нули, проверяем показ/контент/класс/тайминг, не пиксели.
describe('MarkdownPreview — S7 ховер-поповеры', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  const card = (c: HTMLElement) => c.querySelector('[data-popcard]');
  // Прокрутка таймеров + промисов под act (state-апдейт popcard коммитится синхронно к ассерту).
  const advance = (ms: number) => act(async () => void (await vi.advanceTimersByTimeAsync(ms)));

  it('вики: через 220мс показывает popcard с РЕАЛЬНЫМ эксцерптом/типом из заметки', async () => {
    vi.useFakeTimers();
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Цель.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue(
      '---\ntype: idea\nstatus: seed\n---\n# Цель заметки\n\nРеальное тело целевой заметки.',
    );
    vi.spyOn(tauriApi.graph, 'getBacklinks').mockResolvedValue([]);
    const { container } = render(<MarkdownPreview source={'см. [[Цель]]'} onOpenLink={() => {}} />);
    const link = screen.getByText('Цель');

    fireEvent.mouseEnter(link);
    expect(card(container)).toBeNull(); // до 220мс — нет карточки
    await advance(230);
    const pc = card(container);
    expect(pc).not.toBeNull();
    expect(pc?.getAttribute('data-popcard')).toBe('wiki');
    expect(pc?.textContent).toContain('Реальное тело целевой заметки'); // реальный эксцерпт
    expect(pc?.textContent).toContain('Цель заметки'); // заголовок из H1
    expect(pc?.textContent).toContain('IDEA'); // eyebrow = реальный type (uppercase)
  });

  it('вики: mouseleave ДО 220мс → popcard НЕ показывается', async () => {
    vi.useFakeTimers();
    const resolve = vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Цель.md');
    const { container } = render(<MarkdownPreview source={'см. [[Цель]]'} onOpenLink={() => {}} />);
    const link = screen.getByText('Цель');

    fireEvent.mouseEnter(link);
    await advance(100);
    fireEvent.mouseLeave(link); // ушли до срабатывания таймера
    await advance(300);
    expect(card(container)).toBeNull();
    expect(resolve).not.toHaveBeenCalled(); // таймер погашен — резолва даже не было
  });

  it('вики: уход во время async-чтения → popcard НЕ показывается (request-токен от гонок)', async () => {
    vi.useFakeTimers();
    // resolveNote зависает достаточно, чтобы mouseleave успел инвалидировать токен.
    let release: (p: string) => void = () => {};
    vi.spyOn(tauriApi.vault, 'resolveNote').mockImplementation(
      () => new Promise<string>((res) => (release = res)),
    );
    const readFile = vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('тело');
    const { container } = render(<MarkdownPreview source={'см. [[Цель]]'} onOpenLink={() => {}} />);
    const link = screen.getByText('Цель');

    fireEvent.mouseEnter(link);
    await advance(230); // таймер сработал, ушли в resolveNote
    fireEvent.mouseLeave(link); // инвалидируем токен ДО ответа резолва
    await act(async () => {
      release('Notes/Цель.md'); // резолв ответил поздно — должен быть отброшен
      await Promise.resolve();
    });
    expect(card(container)).toBeNull();
    expect(readFile).not.toHaveBeenCalled(); // токен устарел → дальше readFile не пошли
  });

  it('вики: битая ссылка (резолв вернул null) → честное «не найдено», НЕ фейк-превью', async () => {
    vi.useFakeTimers();
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue(null); // заметки нет
    const readFile = vi.spyOn(tauriApi.vault, 'readFile');
    const { container } = render(<MarkdownPreview source={'см. [[Нет такой]]'} onOpenLink={() => {}} />);

    fireEvent.mouseEnter(screen.getByText('Нет такой'));
    await advance(230);
    const pc = card(container);
    expect(pc).not.toBeNull();
    expect(pc?.textContent).toContain('Заметка не найдена'); // честное состояние (ru-локаль)
    expect(readFile).not.toHaveBeenCalled(); // не читаем несуществующий файл
  });

  it('вики .pc-meta: статус + N ссылок — ТОЛЬКО из реальных источников', async () => {
    vi.useFakeTimers();
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Цель.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('---\nstatus: growing\n---\nтело');
    // Реальные беклинки (2 шт.) → счётчик «2 ссылки» из НАСТОЯЩЕГО источника, не выдуман.
    vi.spyOn(tauriApi.graph, 'getBacklinks').mockResolvedValue([
      { sourcePath: 'a.md', sourceTitle: null, context: null, lineNumber: null },
      { sourcePath: 'b.md', sourceTitle: null, context: null, lineNumber: null },
    ]);
    const { container } = render(<MarkdownPreview source={'[[Цель]]'} onOpenLink={() => {}} />);

    fireEvent.mouseEnter(screen.getByText('Цель'));
    await advance(230);
    const pc = card(container);
    expect(pc?.textContent).toContain('growing'); // реальный статус
    expect(pc?.textContent).toMatch(/2 ссылк/); // реальный счётчик беклинков
  });

  it('вики .pc-meta: отсутствует, если нет реального статуса/счётчика (анти-фейк)', async () => {
    vi.useFakeTimers();
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Цель.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('просто тело без frontmatter');
    vi.spyOn(tauriApi.graph, 'getBacklinks').mockResolvedValue([]); // 0 беклинков
    const { container } = render(<MarkdownPreview source={'[[Цель]]'} onOpenLink={() => {}} />);

    fireEvent.mouseEnter(screen.getByText('Цель'));
    await advance(230);
    const pc = card(container);
    expect(pc).not.toBeNull();
    expect(pc?.querySelector('[class*=meta]')).toBeNull(); // нет реальных данных → слот опущен
  });

  it('вики: клик всё ещё зовёт onOpenLink (живая навигация цела)', async () => {
    vi.useFakeTimers();
    const onOpen = vi.fn();
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Цель.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('тело');
    vi.spyOn(tauriApi.graph, 'getBacklinks').mockResolvedValue([]);
    const { container } = render(<MarkdownPreview source={'[[Цель]]'} onOpenLink={onOpen} />);

    fireEvent.mouseEnter(screen.getByText('Цель'));
    await advance(230);
    // Ре-запрос триггера ПОСЛЕ показа карточки (ре-рендер react-markdown мог переcоздать узел ссылки).
    const link = container.querySelector('[data-note]') as HTMLElement;
    await act(async () => void fireEvent.click(link)); // hidePopcard() → setState; зовёт onOpenLink
    expect(onOpen).toHaveBeenCalledWith('Цель'); // навигация не сломана ховером
  });

  it('сноска: через 120мс показывает popcard.fnote с текстом из <li id=…fn-N>', async () => {
    vi.useFakeTimers();
    const { container } = render(
      <MarkdownPreview source={'текст[^1]\n\n[^1]: реальный текст сноски'} onOpenLink={() => {}} />,
    );
    const ref = container.querySelector('sup a') as HTMLElement;
    expect(ref).not.toBeNull();

    fireEvent.mouseEnter(ref);
    expect(card(container)).toBeNull(); // до 120мс — нет
    await advance(130);
    const pc = card(container);
    expect(pc).not.toBeNull();
    expect(pc?.getAttribute('data-popcard')).toBe('fnote');
    expect(pc?.textContent).toContain('реальный текст сноски'); // текст из <li> в DOM
    expect(pc?.textContent).not.toContain('↩'); // backref-стрелка срезана
  });

  it('сноска: mouseleave скрывает popcard', async () => {
    vi.useFakeTimers();
    const { container } = render(
      <MarkdownPreview source={'текст[^1]\n\n[^1]: текст'} onOpenLink={() => {}} />,
    );
    fireEvent.mouseEnter(container.querySelector('sup a') as HTMLElement);
    await advance(130);
    expect(card(container)).not.toBeNull();
    // Ре-запрос ПОСЛЕ показа (ре-рендер мог переcоздать узел) → mouseleave реально на живом триггере.
    const ref = container.querySelector('sup a') as HTMLElement;
    await act(async () => void fireEvent.mouseLeave(ref)); // hidePopcard() → setState
    expect(card(container)).toBeNull();
  });

  it('смена notePath скрывает stale-карточку прежней заметки', async () => {
    vi.useFakeTimers();
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Цель.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('тело');
    vi.spyOn(tauriApi.graph, 'getBacklinks').mockResolvedValue([]);
    const { container, rerender } = render(
      <MarkdownPreview source={'[[Цель]]'} notePath="A.md" onOpenLink={() => {}} />,
    );
    fireEvent.mouseEnter(screen.getByText('Цель'));
    await advance(230);
    expect(card(container)).not.toBeNull();
    rerender(<MarkdownPreview source={'другое'} notePath="B.md" onOpenLink={() => {}} />);
    expect(card(container)).toBeNull(); // карточка прежней заметки убрана
  });

  // FIX 3 (MINOR): живое редактирование (body сменился, тот же notePath) → карточка скрывается
  // (её rect привязан к прежнему DOM → stale-rect).
  it('смена body (живое редактирование) скрывает stale-карточку', async () => {
    vi.useFakeTimers();
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Цель.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('тело');
    vi.spyOn(tauriApi.graph, 'getBacklinks').mockResolvedValue([]);
    const { container, rerender } = render(
      <MarkdownPreview source={'[[Цель]] один'} notePath="A.md" onOpenLink={() => {}} />,
    );
    fireEvent.mouseEnter(screen.getByText('Цель'));
    await advance(230);
    expect(card(container)).not.toBeNull();
    // Та же заметка (notePath A.md), но source изменился — body другой → карточка должна скрыться.
    rerender(<MarkdownPreview source={'[[Цель]] два три'} notePath="A.md" onOpenLink={() => {}} />);
    expect(card(container)).toBeNull();
  });

  // FIX 4 (MINOR): заметка НАЙДЕНА но пустой эксцерпт → «Пустая заметка» (НЕ «не найдена»), title/meta целы.
  it('вики: найденная-но-пустая заметка → «Пустая заметка», НЕ «не найдена» (title/meta сохранены)', async () => {
    vi.useFakeTimers();
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Пустая.md');
    // Только frontmatter + ведущий H1 → эксцерпт тела пуст.
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('---\nstatus: seed\n---\n# Только заголовок\n');
    vi.spyOn(tauriApi.graph, 'getBacklinks').mockResolvedValue([
      { sourcePath: 'a.md', sourceTitle: null, context: null, lineNumber: null },
    ]);
    const { container } = render(<MarkdownPreview source={'[[Пустая]]'} onOpenLink={() => {}} />);

    fireEvent.mouseEnter(screen.getByText('Пустая'));
    await advance(230);
    const pc = card(container);
    expect(pc).not.toBeNull();
    expect(pc?.textContent).toContain('Пустая заметка'); // честное «пустая», а не «не найдена»
    expect(pc?.textContent).not.toContain('не найдена'); // существующая заметка — не лжём «не найдена»
    expect(pc?.textContent).toContain('Только заголовок'); // title из H1 — РЕАЛЬНЫЙ, сохранён
    expect(pc?.textContent).toContain('seed'); // meta-статус — реальный, сохранён
  });

  // FIX 1 (MAJOR): сноска внешнего документа при наличии эмбеда со СВОЕЙ [^1] → показывает текст
  // ВНЕШНЕЙ сноски, не эмбеда (querySelector не должен спускаться в чужой .preview эмбеда).
  it('сноска: при эмбеде со своей [^1] показывает ВНЕШНЮЮ сноску, не сноску эмбеда', async () => {
    vi.useFakeTimers();
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue('Notes/Emb.md');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue(
      'тело эмбеда[^1]\n\n[^1]: ВНУТРЕННЯЯ сноска эмбеда',
    );
    const { container } = render(
      <MarkdownPreview
        notePath="Outer.md"
        source={'![[Emb]]\n\nвнешний текст[^1]\n\n[^1]: ВНЕШНЯЯ сноска документа'}
        onOpenLink={() => {}}
      />,
    );
    // Дать эмбеду резолвиться/прочитаться (его inner-preview с user-content-fn-1 появляется в DOM).
    await advance(0);
    const refs = Array.from(container.querySelectorAll('sup a')) as HTMLElement[];
    expect(refs.length).toBe(2); // ref эмбеда + ref внешнего документа
    const rootPreview = container.querySelector('[class*="preview"]');
    // Внешний footnote-ref — тот, чья ВЛАДЕЮЩАЯ .preview === корневая (не эмбед).
    const outerRef = refs.find((r) => r.closest('[class*="preview"]') === rootPreview) as HTMLElement;
    expect(outerRef).toBeTruthy();

    fireEvent.mouseEnter(outerRef);
    await advance(130);
    const pc = card(container);
    expect(pc).not.toBeNull();
    expect(pc?.textContent).toContain('ВНЕШНЯЯ сноска документа'); // правильная — внешняя
    expect(pc?.textContent).not.toContain('ВНУТРЕННЯЯ'); // НЕ сноска эмбеда (ядро FIX 1)
  });
});
