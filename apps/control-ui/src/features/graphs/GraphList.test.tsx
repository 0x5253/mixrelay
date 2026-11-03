import { fireEvent, render, screen, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type { GraphCatalog } from '../../core/domain/control/catalog';
import { I18nProvider, i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { GraphList } from './GraphList';

const catalog: GraphCatalog = {
  protocolVersion: 1,
  catalogRevision: 1,
  activeId: 'first',
  graphs: [
    { id: 'first', name: 'Broadcast' },
    { id: 'second', name: 'Monitor' },
  ],
  document: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
};

beforeEach(async () => {
  await i18n.changeLanguage('en');
});

function fixture(disabled = false) {
  const onSelect = vi.fn();
  const onDelete = vi.fn();
  const props = { disabled, error: null as string | null };
  const view = () => (
    <MixRelayProvider>
      <I18nProvider>
        <GraphList catalog={catalog} {...props} onSelect={onSelect} onDelete={onDelete} />
      </I18nProvider>
    </MixRelayProvider>
  );
  const rendered = render(view());
  return {
    onSelect,
    onDelete,
    fail: () => {
      props.disabled = true;
      props.error = 'Connection lost';
      rendered.rerender(view());
    },
  };
}

it('allows dismissing an uncertain deletion failure so catalog recovery stays reachable', async () => {
  const f = fixture();
  fireEvent.click(
    within(screen.getByRole('group', { name: 'Broadcast' })).getByRole('button', {
      name: (name) => name.endsWith('Delete graph'),
    }),
  );
  const dialog = await screen.findByRole('dialog');
  fireEvent.click(
    within(dialog).getByRole('button', { name: (name) => name.endsWith('Delete graph') }),
  );
  f.fail();
  expect(within(dialog).getByRole('alert')).toHaveTextContent('Connection lost');
  expect(
    within(dialog).getByRole('button', { name: (name) => name.endsWith('Delete graph') }),
  ).toBeDisabled();
  expect(within(dialog).getByRole('button', { name: 'Cancel' })).toBeEnabled();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  expect(f.onDelete).toHaveBeenCalledTimes(1);
});

it('confirms deletion of the selected graph without selecting or deleting on the first click', async () => {
  const f = fixture();
  const row = screen.getByRole('group', { name: 'Broadcast' });
  expect(within(row).getByRole('button', { name: 'Broadcast' })).toBeDisabled();
  fireEvent.click(
    within(row).getByRole('button', { name: (name) => name.endsWith('Delete graph') }),
  );
  const dialog = await screen.findByRole('dialog', { name: 'Delete graph' });
  expect(within(dialog).getByText(/Broadcast/)).toBeInTheDocument();
  expect(f.onDelete).not.toHaveBeenCalled();
  expect(f.onSelect).not.toHaveBeenCalled();
  fireEvent.click(
    within(dialog).getByRole('button', { name: (name) => name.endsWith('Delete graph') }),
  );
  expect(f.onDelete).toHaveBeenCalledExactlyOnceWith('first');
});

it('cancel leaves a background graph intact and never switches the editor', async () => {
  const f = fixture();
  fireEvent.click(
    within(screen.getByRole('group', { name: 'Monitor' })).getByRole('button', {
      name: (name) => name.endsWith('Delete graph'),
    }),
  );
  fireEvent.click(
    within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }),
  );
  expect(f.onDelete).not.toHaveBeenCalled();
  expect(f.onSelect).not.toHaveBeenCalled();
});

it('does not open deletion while another writer is busy', () => {
  fixture(true);
  for (const button of screen.getAllByRole('button', {
    name: (name) => name.endsWith('Delete graph'),
  })) {
    expect(button).toBeDisabled();
    fireEvent.click(button);
  }
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('selects a background graph through its owner without opening deletion', () => {
  const f = fixture();
  fireEvent.click(screen.getByRole('button', { name: 'Monitor' }));
  expect(f.onSelect).toHaveBeenCalledExactlyOnceWith('second');
  expect(f.onDelete).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
