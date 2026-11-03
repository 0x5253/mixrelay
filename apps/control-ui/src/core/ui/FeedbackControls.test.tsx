import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import themeStyles from '../theme/ComponentTheme.module.css';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { ActionButton } from './ActionButton';
import { InfoPopover } from './InfoPopover';
import { NameDialog } from './NameDialog';
import { SelectField } from './SelectField';
import { StatusIndicator } from './StatusIndicator';

it.each([false, true])(
  'opens informational feedback, then restores its trigger (critical=%s)',
  async (critical) => {
    render(
      <MixRelayProvider>
        <InfoPopover label="Audio details" critical={critical}>
          <p>Last confirmed settings: 48 kHz · 128 frames</p>
          <ActionButton>Retry connection</ActionButton>
        </InfoPopover>
      </MixRelayProvider>,
    );
    const trigger = screen.getByRole('button', { name: 'Audio details' });
    trigger.focus();
    expect(trigger).toHaveAttribute('data-tone', critical ? 'critical' : 'neutral');
    fireEvent.click(trigger);
    const dialog = await screen.findByRole('dialog', { name: 'Audio details' });
    expect(trigger).toHaveAttribute('aria-expanded', 'true');
    expect(trigger).toHaveAttribute('aria-controls', dialog.id);
    await waitFor(() => expect(dialog).toHaveFocus());
    fireEvent.keyDown(dialog, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    await waitFor(() => expect(trigger).toHaveFocus());
  },
);

it('uses the shared floating layer and escapes a clipped field container', async () => {
  const change = vi.fn();
  const view = render(
    <MixRelayProvider>
      <div data-testid="clipped">
        <SelectField
          label="Device"
          value="io"
          data={[
            { value: 'io', label: 'MixRelay IO' },
            { value: 'monitor', label: 'Monitor' },
          ]}
          onChange={change}
        />
      </div>
    </MixRelayProvider>,
  );
  const field = screen.getByRole('button', { name: /Device$/ });
  field.focus();
  fireEvent.click(field);
  const option = await screen.findByRole('option', { name: 'Monitor' });
  expect(screen.getByTestId('clipped')).not.toContainElement(option);
  expect(option.closest('[data-placement]')).toHaveClass(themeStyles.selectPopover ?? '');
  fireEvent.click(option);
  expect(change).toHaveBeenCalledExactlyOnceWith('monitor');
  expect(field).toHaveTextContent('MixRelay IO');
  view.rerender(
    <MixRelayProvider>
      <SelectField label="Device" value={null} data={[]} disabled onChange={change} />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('button', { name: /Device$/ })).toBeDisabled();
});

it('uses closed-field keyboard selection and skips disabled choices without changing readback', () => {
  const change = vi.fn();
  render(
    <MixRelayProvider>
      <SelectField
        label="Layout"
        value="mono"
        data={[
          { value: 'mono', label: 'Mono' },
          { value: 'surround', label: 'Surround', disabled: true },
          { value: 'stereo', label: 'Stereo' },
        ]}
        onChange={change}
      />
    </MixRelayProvider>,
  );
  const field = screen.getByRole('button', { name: 'Mono Layout' });
  field.focus();
  fireEvent.keyDown(field, { key: 'ArrowRight' });
  expect(change).toHaveBeenCalledExactlyOnceWith('stereo');
  expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  expect(field).toHaveTextContent('Mono');
  expect(field).toHaveFocus();
});

it('uses typeahead from the closed field to request a named choice', () => {
  const change = vi.fn();
  render(
    <MixRelayProvider>
      <SelectField
        label="Layout"
        value="mono"
        data={[
          { value: 'mono', label: 'Mono' },
          { value: 'stereo', label: 'Stereo' },
        ]}
        onChange={change}
      />
    </MixRelayProvider>,
  );
  const field = screen.getByRole('button', { name: 'Mono Layout' });
  field.focus();
  fireEvent.keyDown(field, { key: 's' });
  expect(change).toHaveBeenCalledExactlyOnceWith('stereo');
  expect(field).toHaveTextContent('Mono');
});

it('connects the selected mode explanation without adding another keyboard stop', () => {
  const renderMode = (value: string, description?: string) => (
    <MixRelayProvider>
      <SelectField
        label="Channel mode"
        value={value}
        description={description}
        data={[
          { value: 'mono', label: 'Mono sum' },
          { value: 'left', label: 'Duplicate left' },
        ]}
        onChange={vi.fn()}
      />
    </MixRelayProvider>
  );
  const view = render(renderMode('mono', '(L + R) / 2 → L/R'));
  const field = screen.getByRole('button', { name: 'Mono sum Channel mode' });
  expect(field).toHaveAccessibleDescription('(L + R) / 2 → L/R');
  expect(screen.getByText('(L + R) / 2 → L/R')).not.toHaveAttribute('tabindex');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  view.rerender(renderMode('left', 'L → L/R'));
  expect(screen.getByRole('button', { name: 'Duplicate left Channel mode' })).toBe(field);
  expect(field).toHaveAccessibleDescription('L → L/R');
  view.rerender(renderMode('left'));
  expect(field).toHaveAccessibleDescription('');
});

it('keeps a choice explanation tied to confirmed readback while another mode is requested', () => {
  const change = vi.fn();
  const mode = (value: string) => (
    <MixRelayProvider>
      <SelectField
        label="Channel mode"
        value={value}
        data={[
          { value: 'mono', label: 'Mono sum', description: '(L + R) / 2 → L/R' },
          { value: 'left', label: 'Duplicate left', description: 'L → L/R' },
        ]}
        onChange={change}
      />
    </MixRelayProvider>
  );
  const view = render(mode('mono'));
  const field = screen.getByRole('button', { name: 'Mono sum Channel mode' });
  field.focus();
  fireEvent.keyDown(field, { key: 'ArrowRight' });
  expect(change).toHaveBeenCalledExactlyOnceWith('left');
  expect(field).toHaveTextContent('Mono sum');
  expect(field).toHaveAccessibleDescription('(L + R) / 2 → L/R');
  view.rerender(mode('left'));
  expect(field).toHaveTextContent('Duplicate left');
  expect(field).toHaveAccessibleDescription('L → L/R');
});

it('retries a selected value once by pointer and keyboard', async () => {
  const change = vi.fn();
  render(
    <MixRelayProvider>
      <SelectField
        label="Buffer size"
        value="128"
        data={[
          { value: '128', label: '128 frames' },
          { value: '256', label: '256 frames' },
        ]}
        onChange={change}
      />
    </MixRelayProvider>,
  );
  const field = screen.getByRole('button', { name: /Buffer size$/ });
  fireEvent.click(field);
  fireEvent.click(await screen.findByRole('option', { name: '128 frames' }));
  expect(change).toHaveBeenCalledExactlyOnceWith('128');
  fireEvent.click(field);
  const selected = await screen.findByRole('option', { name: '128 frames' });
  selected.focus();
  fireEvent.keyDown(selected, { key: 'Enter' });
  fireEvent.keyUp(selected, { key: 'Enter' });
  expect(change.mock.calls).toEqual([['128'], ['128']]);
});

it.each(['neutral', 'positive', 'warning', 'critical'] as const)(
  'names the %s status marker without adding a keyboard stop',
  (tone) => {
    render(
      <MixRelayProvider>
        <StatusIndicator label="Driver unavailable" tone={tone} />
      </MixRelayProvider>,
    );
    const marker = screen.getByRole('img', { name: 'Driver unavailable' });
    expect(marker).toHaveAttribute('data-tone', tone);
    expect(marker).toHaveAttribute('tabindex', '-1');
    expect(marker).not.toHaveAttribute('aria-live');
  },
);

it('rejects empty or locked name submissions and presents the recovery message', () => {
  const create = vi.fn();
  const cancel = vi.fn();
  const renderDialog = (enabled: boolean) => (
    <MixRelayProvider>
      <NameDialog
        title="New device"
        initialName="   "
        nameLabel="Name"
        createLabel="Create"
        enabled={enabled}
        onCreate={create}
        onCancel={cancel}
        error="Device could not be created. Try again."
      />
    </MixRelayProvider>
  );
  const view = render(renderDialog(true));
  const input = screen.getByRole('textbox', { name: 'Name' });
  const form = input.closest('form');
  if (!form) throw new Error('Name dialog must use a native form');
  fireEvent.submit(form);
  expect(create).not.toHaveBeenCalled();
  expect(screen.getByRole('alert')).toHaveTextContent('Try again');
  fireEvent.change(input, { target: { value: '  Studio IO  ' } });
  fireEvent.submit(form);
  expect(create).toHaveBeenCalledExactlyOnceWith('Studio IO');
  view.rerender(renderDialog(false));
  fireEvent.submit(form);
  expect(create).toHaveBeenCalledOnce();
  expect(input).toBeDisabled();
});
