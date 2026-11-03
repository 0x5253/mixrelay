import { fireEvent, render, screen } from '@testing-library/react';
import { userEvent } from 'storybook/test';
import { afterEach, expect, it, vi } from 'vitest';

afterEach(() => vi.restoreAllMocks());

import { I18nProvider } from '../i18n';
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

it('reveals and hides a password without submitting or changing the secret', async () => {
  const submit = vi.fn();
  const change = vi.fn();
  const field = (disabled = false) => (
    <MixRelayProvider>
      <I18nProvider>
        <form onSubmit={submit}>
          <TextField
            label="Licence key"
            type="password"
            value="test-key"
            onChange={change}
            disabled={disabled}
          />
        </form>
      </I18nProvider>
    </MixRelayProvider>
  );
  const view = render(field());
  const input = screen.getByLabelText('Licence key');
  expect(input).toHaveAttribute('type', 'password');
  fireEvent.click(await screen.findByRole('button', { name: 'Licence key: Show password' }));
  expect(input).toHaveAttribute('type', 'text');
  expect(screen.getByRole('button', { name: 'Licence key: Hide password' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  fireEvent.click(screen.getByRole('button', { name: 'Licence key: Hide password' }));
  expect(input).toHaveAttribute('type', 'password');
  expect(input).toHaveValue('test-key');
  expect(change).not.toHaveBeenCalled();
  expect(submit).not.toHaveBeenCalled();
  view.rerender(field(true));
  expect(input).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Licence key: Show password' })).toBeDisabled();
});

it('owns uncontrolled password defaults in the field context and retains edits across reveal and rerender without control warnings', async () => {
  const errors = vi.spyOn(console, 'error');
  const changed: string[] = [];
  const user = userEvent.setup();
  const field = (defaultValue: string) => (
    <MixRelayProvider>
      <I18nProvider>
        <TextField
          label="Default key"
          type="password"
          defaultValue={defaultValue}
          onChange={(event) => changed.push(event.currentTarget.value)}
        />
      </I18nProvider>
    </MixRelayProvider>
  );
  const view = render(field('example-key'));
  const input = screen.getByLabelText('Default key');
  expect(input).toHaveValue('example-key');
  await user.click(screen.getByRole('button', { name: 'Default key: Show password' }));
  expect(input).toHaveAttribute('type', 'text');
  await user.clear(input);
  await user.type(input, 'edited-key');
  expect(changed.at(-1)).toBe('edited-key');
  await user.click(screen.getByRole('button', { name: 'Default key: Hide password' }));
  expect(input).toHaveAttribute('type', 'password');
  expect(input).toHaveValue('edited-key');
  view.rerender(field('different-default'));
  expect(screen.getByLabelText('Default key')).toBe(input);
  expect(input).toHaveValue('edited-key');
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
