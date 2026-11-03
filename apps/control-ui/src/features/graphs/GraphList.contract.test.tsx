import { act, fireEvent, render, screen, within } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { expect, it, vi } from 'vitest';
import type { GraphCatalog } from '../../core/domain/control/catalog';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import type { ConfirmationDialog } from '../../core/ui/ConfirmationDialog';
import { GraphList } from './GraphList';

const confirmation = vi.hoisted(() => ({
  current: null as ComponentProps<typeof ConfirmationDialog> | null,
}));
vi.mock('../../core/ui/ConfirmationDialog', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../core/ui/ConfirmationDialog')>();
  return {
    ...actual,
    ConfirmationDialog: (props: ComponentProps<typeof ConfirmationDialog>) => {
      confirmation.current = props;
      return <actual.ConfirmationDialog {...props} />;
    },
  };
});

const catalog: GraphCatalog = {
  protocolVersion: 1,
  catalogRevision: 1,
  activeId: 'first',
  graphs: [{ id: 'first', name: 'Broadcast' }],
  document: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
};

function currentConfirmation() {
  if (!confirmation.current) throw new Error('Confirmation owner must have rendered');
  return confirmation.current;
}

it('keeps the deletion confirmation while a native write is busy and permits dismissal after it settles', async () => {
  await i18n.changeLanguage('en');
  const props = {
    catalog,
    disabled: false,
    busy: false,
    error: null,
    onSelect: vi.fn(),
    onDelete: vi.fn(),
  };
  const content = () => (
    <MixRelayProvider>
      <GraphList {...props} />
    </MixRelayProvider>
  );
  const view = render(content());
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Delete graph') }));
  const dialog = await screen.findByRole('dialog');
  props.busy = true;
  view.rerender(content());
  expect(within(dialog).getByRole('button', { name: 'Cancel' })).toBeDisabled();
  const confirm = within(dialog).getByRole('button', { name: 'Delete graph' });
  expect(confirm).toBeDisabled();
  expect(confirm).toHaveAttribute('aria-busy', 'true');
  fireEvent.click(confirm);
  expect(props.onDelete).not.toHaveBeenCalled();
  act(() => currentConfirmation().onCancel());
  expect(screen.getByRole('dialog')).toBe(dialog);
  expect(props.onDelete).not.toHaveBeenCalled();
  props.busy = false;
  view.rerender(content());
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('ignores confirmation when a native catalog refresh has already removed its target', async () => {
  await i18n.changeLanguage('en');
  const onDelete = vi.fn();
  const content = (value: GraphCatalog) => (
    <MixRelayProvider>
      <GraphList
        catalog={value}
        disabled={false}
        error={null}
        onSelect={vi.fn()}
        onDelete={onDelete}
      />
    </MixRelayProvider>
  );
  const view = render(content(catalog));
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Delete graph') }));
  await screen.findByRole('dialog');
  view.rerender(content({ ...catalog, graphs: [] }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  act(() => currentConfirmation().onConfirm());
  expect(onDelete).not.toHaveBeenCalled();
});
