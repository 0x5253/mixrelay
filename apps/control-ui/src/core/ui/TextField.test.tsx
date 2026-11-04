import { fireEvent, render, screen } from '@testing-library/react';
import { userEvent } from 'storybook/test';
import { afterEach, expect, it, vi } from 'vitest';

afterEach(() => vi.restoreAllMocks());

import { MixRelayProvider } from '../theme/MixRelayProvider';
import { TextField } from './TextField';

it('connects field descriptions and errors without replacing the controlled input', () => {
  const change = vi.fn();
  const field = (error?: string) => (
    <MixRelayProvider>
      <TextField
        label="Device name"
        value="Studio"
        description="Used in audio apps"
        error={error}
        required
        onChange={change}
      />
    </MixRelayProvider>
  );
  const view = render(field());
  const input = screen.getByRole('textbox', { name: 'Device name' });
  expect(input).toBeRequired();
  expect(input).toHaveAccessibleDescription('Used in audio apps');
  fireEvent.change(input, { target: { value: 'Monitor' } });
  expect(change).toHaveBeenCalledOnce();
  expect(input).toHaveValue('Studio');
  view.rerender(field('Name is already in use'));
  expect(screen.getByRole('textbox')).toBe(input);
  expect(input).toHaveAttribute('aria-invalid', 'true');
  expect(screen.getByRole('alert')).toHaveTextContent('Name is already in use');
  view.rerender(field());
  expect(input).not.toHaveAttribute('aria-invalid', 'true');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

it('names a read-only field and focuses the requested input', () => {
  render(
    <MixRelayProvider>
      <TextField aria-label="Saved path" defaultValue="/Music" readOnly data-autofocus />
    </MixRelayProvider>,
  );
  const input = screen.getByRole('textbox', { name: 'Saved path' });
  expect(input).toHaveAttribute('readonly');
  expect(input).toHaveValue('/Music');
  expect(input).toHaveFocus();
});

it('owns uncontrolled defaults in the field context and retains edits across rerender without control warnings', async () => {
  const errors = vi.spyOn(console, 'error');
  const changed: string[] = [];
  const user = userEvent.setup();
  const field = (defaultValue: string) => (
    <MixRelayProvider>
      <TextField
        label="Default name"
        defaultValue={defaultValue}
        onChange={(event) => changed.push(event.currentTarget.value)}
      />
    </MixRelayProvider>
  );
  const view = render(field('Studio'));
  const input = screen.getByRole('textbox', { name: 'Default name' });
  expect(input).toHaveValue('Studio');
  await user.clear(input);
  await user.type(input, 'Edited');
  expect(changed.at(-1)).toBe('Edited');
  view.rerender(field('Different default'));
  expect(screen.getByRole('textbox', { name: 'Default name' })).toBe(input);
  expect(input).toHaveValue('Edited');
  const controlWarnings = errors.mock.calls.filter((call) =>
    call.some(
      (part) =>
        typeof part === 'string' && /controlled|uncontrolled|value.*defaultValue/i.test(part),
    ),
  );
  expect(controlWarnings).toEqual([]);
});

it.each([
  ['numeric', 42, '42'],
  ['array', ['One', 'Two'], 'One,Two'],
] as const)(
  'preserves native text default coercion for %s defaults',
  (_kind, defaultValue, expected) => {
    render(
      <MixRelayProvider>
        <TextField label="Default text" defaultValue={defaultValue} />
      </MixRelayProvider>,
    );
    expect(screen.getByRole('textbox', { name: 'Default text' })).toHaveValue(expected);
  },
);
