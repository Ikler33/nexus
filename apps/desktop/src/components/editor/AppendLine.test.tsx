import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AppendLine } from './AppendLine';
import { type NoteRef } from '../../lib/tauri-api';

afterEach(() => vi.restoreAllMocks());

const noNotes = async (): Promise<NoteRef[]> => [];

describe('AppendLine (editor-chrome)', () => {
  it('Enter дописывает введённую строку через onAppend и очищает инпут', () => {
    const onAppend = vi.fn();
    render(<AppendLine onAppend={onAppend} fetchNotes={noNotes} />);
    const input = screen.getByRole('textbox') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '  новая задача  ' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onAppend).toHaveBeenCalledWith('новая задача'); // trim
    expect(input.value).toBe('');
  });

  it('пустой ввод не дописывает (Enter на пробелах — no-op)', () => {
    const onAppend = vi.fn();
    render(<AppendLine onAppend={onAppend} fetchNotes={noNotes} />);
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: '   ' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onAppend).not.toHaveBeenCalled();
  });

  it('[[ открывает автокомплит (fetchNotes), выбор вставляет [[Note]] и не дописывает строку', async () => {
    const onAppend = vi.fn();
    const fetchNotes = vi.fn(
      async (): Promise<NoteRef[]> => [{ path: 'Notes/Ideas.md', title: 'Ideas' }],
    );
    render(<AppendLine onAppend={onAppend} fetchNotes={fetchNotes} />);
    const input = screen.getByRole('textbox') as HTMLInputElement;
    fireEvent.change(input, {
      target: { value: 'см. [[Id', selectionStart: 8 },
    });
    // Поп-ап подтянул совпадение через fetchNotes
    const option = await screen.findByRole('option', { name: /Ideas/ });
    expect(fetchNotes).toHaveBeenCalledWith('Id');
    // Выбор мышью вставляет [[Ideas]] (title по name заметки) вместо набранного [[Id
    fireEvent.mouseDown(option);
    expect(input.value).toBe('см. [[Ideas]]');
    expect(onAppend).not.toHaveBeenCalled(); // выбор ссылки — не отправка строки
  });

  it('[[ + Enter выбирает выделенный пункт автокомплита (не дописывает)', async () => {
    const onAppend = vi.fn();
    const fetchNotes = vi.fn(
      async (): Promise<NoteRef[]> => [{ path: 'Alpha.md', title: 'Alpha' }],
    );
    render(<AppendLine onAppend={onAppend} fetchNotes={fetchNotes} />);
    const input = screen.getByRole('textbox') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '[[Al', selectionStart: 4 } });
    await screen.findByRole('option', { name: /Alpha/ });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(input.value).toBe('[[Alpha]]');
    expect(onAppend).not.toHaveBeenCalled();
  });

  it('Escape закрывает поп-ап автокомплита', async () => {
    const fetchNotes = vi.fn(
      async (): Promise<NoteRef[]> => [{ path: 'Beta.md', title: 'Beta' }],
    );
    render(<AppendLine onAppend={vi.fn()} fetchNotes={fetchNotes} />);
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: '[[Be', selectionStart: 4 } });
    await screen.findByRole('option', { name: /Beta/ });
    fireEvent.keyDown(input, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('option')).toBeNull());
  });
});
