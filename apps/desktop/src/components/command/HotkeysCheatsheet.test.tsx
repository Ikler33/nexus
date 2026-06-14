import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { HotkeysCheatsheet } from './HotkeysCheatsheet';
import { commands, type Disposable } from '../../lib/commands';
import { useUIStore } from '../../stores/ui';

const disposers: Disposable[] = [];

function registerTestCommands() {
  // id-префиксы кладут команды в нужные секции; одна без defaultKey — должна выпасть из карты.
  disposers.push(
    commands.register({ id: 'nav.tback', title: 'Go back', source: 'core', defaultKey: 'mod+[', run() {} }),
    commands.register({ id: 'file.tnew', title: 'New note', source: 'core', defaultKey: 'mod+n', run() {} }),
    commands.register({
      id: 'editor.tbold',
      title: 'Bold text',
      source: 'core',
      defaultKey: 'mod+b',
      run() {},
    }),
    commands.register({ id: 'view.tgraph', title: 'Graph', source: 'core', defaultKey: 'mod+g', run() {} }),
    commands.register({ id: 'theme.tnokey', title: 'No hotkey here', source: 'core', run() {} }),
  );
}

afterEach(() => {
  disposers.splice(0).forEach((d) => d.dispose());
  useUIStore.setState({
    cheatsheetOpen: false,
    paletteOpen: false,
    tasksOpen: false,
    goalsOpen: false,
    inboxOpen: false,
  });
});

describe('HotkeysCheatsheet (POLISH ⌘/)', () => {
  it('закрыта → ничего не рендерит', () => {
    useUIStore.setState({ cheatsheetOpen: false });
    const { container } = render(<HotkeysCheatsheet />);
    expect(container).toBeEmptyDOMElement();
  });

  it('открыта → группирует команды С хоткеем, команду без хоткея НЕ показывает', () => {
    registerTestCommands();
    useUIStore.setState({ cheatsheetOpen: true });
    render(<HotkeysCheatsheet />);

    // Заголовок + команды с сочетаниями по секциям.
    expect(screen.getByText('Горячие клавиши')).toBeInTheDocument();
    expect(screen.getByText('Go back')).toBeInTheDocument();
    expect(screen.getByText('New note')).toBeInTheDocument();
    expect(screen.getByText('Bold text')).toBeInTheDocument();
    expect(screen.getByText('Graph')).toBeInTheDocument();
    // Заголовки секций (локаль тестов — ru, см. test/setup.ts).
    expect(screen.getByText('Навигация')).toBeInTheDocument();
    expect(screen.getByText('Редактор')).toBeInTheDocument();
    // Команда без defaultKey не попадает в карту хоткеев.
    expect(screen.queryByText('No hotkey here')).not.toBeInTheDocument();
  });

  it('показывает форматированное сочетание (kbd) для команды', () => {
    registerTestCommands();
    useUIStore.setState({ cheatsheetOpen: true });
    render(<HotkeysCheatsheet />);
    // formatCombo для mod+b даёт ⌘B (mac) или Ctrl+B — в обоих есть «B».
    const boldRow = screen.getByText('Bold text').closest('li');
    expect(boldRow?.querySelector('kbd')?.textContent).toMatch(/B/);
  });

  it('Esc закрывает (cheatsheetOpen → false)', () => {
    useUIStore.setState({ cheatsheetOpen: true });
    render(<HotkeysCheatsheet />);
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(useUIStore.getState().cheatsheetOpen).toBe(false);
  });

  it('кнопка ✕ закрывает', () => {
    useUIStore.setState({ cheatsheetOpen: true });
    render(<HotkeysCheatsheet />);
    fireEvent.click(screen.getByRole('button', { name: /закрыть|close/i }));
    expect(useUIStore.getState().cheatsheetOpen).toBe(false);
  });

  // Главный риск среза (adversarial-ревью): шпаргатка не должна стекаться с другими focus-trap.
  it('взаимоисключение: openCheatsheet гасит палитру/Tasks/Goals/Inbox', () => {
    useUIStore.setState({
      paletteOpen: true,
      tasksOpen: true,
      goalsOpen: true,
      inboxOpen: true,
      cheatsheetOpen: false,
    });
    useUIStore.getState().openCheatsheet();
    const s = useUIStore.getState();
    expect(s.cheatsheetOpen).toBe(true);
    expect(s.paletteOpen).toBe(false);
    expect(s.tasksOpen).toBe(false);
    expect(s.goalsOpen).toBe(false);
    expect(s.inboxOpen).toBe(false);
  });

  it('симметрия: открытие Tasks или палитры гасит шпаргатку', () => {
    useUIStore.setState({ cheatsheetOpen: true, tasksOpen: false });
    useUIStore.getState().toggleTasks();
    expect(useUIStore.getState().cheatsheetOpen).toBe(false);
    expect(useUIStore.getState().tasksOpen).toBe(true);

    useUIStore.setState({ cheatsheetOpen: true, paletteOpen: false });
    useUIStore.getState().openPalette();
    expect(useUIStore.getState().cheatsheetOpen).toBe(false);
    expect(useUIStore.getState().paletteOpen).toBe(true);
  });
});
