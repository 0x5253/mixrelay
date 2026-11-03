import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { i18n } from '../i18n';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { Dialog } from './Dialog';
import { TextField } from './TextField';

it('blocks dismissal while a command is in progress and enables it after completion', () => {
  const onClose = vi.fn();
  const view = (busy: boolean) => (
    <MixRelayProvider>
      <Dialog opened title="Device" closeLabel="Close dialog" busy={busy} onClose={onClose}>
        <p>Applying settings</p>
      </Dialog>
    </MixRelayProvider>
  );
  const { rerender } = render(view(true));
  expect(screen.getByRole('button', { name: 'Close dialog' })).toBeDisabled();
  fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
  expect(onClose).not.toHaveBeenCalled();
  rerender(view(false));
  expect(screen.getByRole('button', { name: 'Close dialog' })).toBeEnabled();
  fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('labels the dialog and moves initial focus to its marked field', async () => {
  const onClose = vi.fn();
  render(
    <MixRelayProvider>
      <Dialog opened title="New graph" onClose={onClose}>
        <TextField data-autofocus label="Graph name" defaultValue="Studio" />
      </Dialog>
    </MixRelayProvider>,
  );
  expect(screen.getByRole('dialog', { name: 'New graph' })).toHaveAttribute('aria-modal', 'true');
  expect(screen.getByRole('textbox', { name: 'Graph name' })).toHaveValue('Studio');
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Graph name' })).toHaveFocus());
  fireEvent.click(screen.getByRole('button', { name: i18n.t('common.cancel') }));
  expect(onClose).toHaveBeenCalledOnce();
});

it('restores the launcher after closing a dialog mounted already open', async () => {
  function Workflow() {
    const [opened, setOpened] = useState(false);
    return (
      <MixRelayProvider>
        <button type="button" onClick={() => setOpened(true)}>
          New graph
        </button>
        {opened ? (
          <Dialog opened title="Graph" closeLabel="Cancel" onClose={() => setOpened(false)}>
            <TextField data-autofocus label="Name" />
          </Dialog>
        ) : null}
      </MixRelayProvider>
    );
  }
  render(<Workflow />);
  const launcher = screen.getByRole('button', { name: 'New graph' });
  launcher.focus();
  fireEvent.click(launcher);
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Name' })).toHaveFocus());
  fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
  await waitFor(() => expect(launcher).toHaveFocus());
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('preserves focus when the workflow has already moved it to another control', async () => {
  function Workflow() {
    const [opened, setOpened] = useState(false);
    return (
      <MixRelayProvider>
        <button type="button" onClick={() => setOpened(true)}>
          Open
        </button>
        <button type="button">Next control</button>
        {opened ? (
          <Dialog opened title="Graph" closeLabel="Cancel" onClose={() => setOpened(false)}>
            <TextField data-autofocus label="Name" />
          </Dialog>
        ) : null}
      </MixRelayProvider>
    );
  }
  render(<Workflow />);
  const launcher = screen.getByRole('button', { name: 'Open' });
  launcher.focus();
  fireEvent.click(launcher);
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Name' })).toHaveFocus());
  const nextControl = screen.getByRole('button', { name: 'Next control', hidden: true });
  nextControl.focus();
  fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(nextControl).toHaveFocus();
});
