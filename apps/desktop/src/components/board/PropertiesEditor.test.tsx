import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { PropertiesEditor } from './PropertiesEditor';
import i18n from '../../i18n/setup';
import { tauriApi, type NoteProperty } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';

const PROPS: NoteProperty[] = [
  { key: 'status', value: 'todo', type: 'text' },
  { key: 'done', value: 'false', type: 'checkbox' },
  { key: 'due', value: 'скоро', type: 'date' }, // значение НЕ под типом → invalid
];

describe('PropertiesEditor (PROP-3 — инлайн-правка свойств, §7)', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en');
    vi.restoreAllMocks();
    useWorkspaceStore.setState({ buffers: {} });
    vi.spyOn(tauriApi.properties, 'forNote').mockResolvedValue(PROPS.map((p) => ({ ...p })));
  });

  it('правка text-свойства → set_frontmatter_field(status) + onChanged', async () => {
    const setFm = vi
      .spyOn(tauriApi.vault, 'setFrontmatterField')
      .mockResolvedValue({ content: '---\nstatus: doing\n---\n', hash: 'h2' });
    const onChanged = vi.fn();
    render(<PropertiesEditor path="Tasks/T.md" onOpenSource={() => {}} onChanged={onChanged} />);

    const input = await screen.findByLabelText('status');
    fireEvent.change(input, { target: { value: 'doing' } });
    fireEvent.blur(input);

    await waitFor(() => expect(setFm).toHaveBeenCalledWith('Tasks/T.md', 'status', 'doing'));
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it('чекбокс-свойство → set_frontmatter_field(true)', async () => {
    const setFm = vi
      .spyOn(tauriApi.vault, 'setFrontmatterField')
      .mockResolvedValue({ content: '---\ndone: true\n---\n', hash: 'h3' });
    render(<PropertiesEditor path="Tasks/T.md" onOpenSource={() => {}} />);
    fireEvent.click(await screen.findByLabelText('done'));
    await waitFor(() => expect(setFm).toHaveBeenCalledWith('Tasks/T.md', 'done', 'true'));
  });

  it('значение не под типом (date←«скоро») → invalid-поле + «Edit in source»', async () => {
    const onOpenSource = vi.fn();
    render(<PropertiesEditor path="Tasks/T.md" onOpenSource={onOpenSource} />);
    const btn = await screen.findByRole('button', { name: /Edit in source/i });
    expect(screen.getByText('скоро')).toBeInTheDocument();
    fireEvent.click(btn);
    expect(onOpenSource).toHaveBeenCalled();
  });
});
