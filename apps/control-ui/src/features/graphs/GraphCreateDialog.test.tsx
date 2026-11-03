import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { GraphCreateDialog } from './GraphCreateDialog';

it('retains an unsuccessful graph name and displays the owner error for an explicit retry', async () => {
  await i18n.changeLanguage('en');
  const create = vi.fn();
  render(
    <MixRelayProvider>
      <GraphCreateDialog
        suggestedName="  Broadcast  "
        onCreate={create}
        onCancel={vi.fn()}
        error="The graph could not be saved."
      />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('alert')).toHaveTextContent('The graph could not be saved.');
  expect(screen.getByRole('textbox', { name: 'Graph name' })).toHaveValue('  Broadcast  ');
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  expect(create).toHaveBeenCalledExactlyOnceWith('Broadcast');
});

it('blocks a blank form submission and leaves cancellation available without an owner error', async () => {
  await i18n.changeLanguage('en');
  const create = vi.fn();
  const cancel = vi.fn();
  render(
    <MixRelayProvider>
      <GraphCreateDialog suggestedName="New graph" onCreate={create} onCancel={cancel} />
    </MixRelayProvider>,
  );
  const field = screen.getByRole('textbox', { name: 'Graph name' });
  fireEvent.change(field, { target: { value: '  ' } });
  expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
  const form = field.closest('form');
  if (!form) throw new Error('Graph name must belong to its creation form');
  fireEvent.submit(form);
  expect(create).not.toHaveBeenCalled();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(cancel).toHaveBeenCalledOnce();
});
