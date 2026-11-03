import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { ApplicationStatusBar } from './ApplicationStatusBar';
import { RecoveryActions } from './RecoveryActions';
import { UpdatesDialog } from './UpdatesDialog';
import { WorkspaceHeader } from './WorkspaceHeader';

it('places app information and the global action in one footer row', () => {
  render(
    <ApplicationStatusBar
      information={<span>0.1.0</span>}
      controls={<button type="button">Pause</button>}
    />,
  );
  const footer = screen.getByRole('contentinfo');
  expect(footer).toHaveTextContent('0.1.0');
  expect(screen.getByRole('button', { name: 'Pause' })).toBeInTheDocument();
  expect(footer.querySelectorAll('[tabindex]')).toHaveLength(0);
});

it('keeps an empty information area when only the global action is shown', () => {
  render(<ApplicationStatusBar controls={<button type="button">Resume</button>} />);
  expect(screen.getByRole('contentinfo').firstElementChild).toBeEmptyDOMElement();
});

it('offers retry and reload as two ordinary commands in that order', () => {
  const onRetry = vi.fn();
  const onReload = vi.fn();
  render(
    <MixRelayProvider>
      <RecoveryActions
        retryLabel="Retry"
        reloadLabel="Reload"
        onRetry={onRetry}
        onReload={onReload}
      />
    </MixRelayProvider>,
  );
  const [retry, reload] = screen.getAllByRole('button');
  expect(retry).toHaveAccessibleName('Retry');
  expect(reload).toHaveAccessibleName('Reload');
  fireEvent.click(retry as HTMLElement);
  fireEvent.click(reload as HTMLElement);
  expect(onRetry).toHaveBeenCalledOnce();
  expect(onReload).toHaveBeenCalledOnce();
});

it('frames update details in the shared dialog with a header close action', () => {
  const onClose = vi.fn();
  render(
    <MixRelayProvider>
      <UpdatesDialog opened title="Updates" closeLabel="Close updates" onClose={onClose}>
        <p>MixRelay is up to date.</p>
      </UpdatesDialog>
    </MixRelayProvider>,
  );
  expect(screen.getByRole('dialog', { name: 'Updates' })).toHaveTextContent(
    'MixRelay is up to date.',
  );
  fireEvent.click(screen.getByRole('button', { name: 'Close updates' }));
  expect(onClose).toHaveBeenCalledOnce();
});

it('wraps workspace titles in one header landmark without adding a Tab stop', () => {
  render(
    <WorkspaceHeader>
      <h2>Untitled Graph</h2>
    </WorkspaceHeader>,
  );
  const header = screen.getByRole('banner');
  expect(header).toContainElement(screen.getByRole('heading', { name: 'Untitled Graph' }));
  expect(header).not.toHaveAttribute('tabindex');
});
