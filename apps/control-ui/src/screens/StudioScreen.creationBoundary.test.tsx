import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import '../test-support/integration';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { I18nProvider, i18n } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import type { ActionButton } from '../core/ui/ActionButton';
import { StudioScreen } from './StudioScreen';

const commands = vi.hoisted(() => ({ create: null as (() => void) | null }));
// Keep the actual Core button. Capture the delegated command to verify the parent
// policy also protects a late command from another input surface, beyond disabled DOM.
vi.mock('../core/ui/ActionButton', async (original) => {
  const actual = await original<typeof import('../core/ui/ActionButton')>();
  return {
    ...actual,
    ActionButton: (props: ComponentProps<typeof ActionButton>) => {
      if (props['aria-keyshortcuts']) commands.create = () => props.onClick?.(undefined as never);
      return <actual.ActionButton {...props} />;
    },
  };
});
vi.mock('../features/routing-workspace/RoutingWorkspace', () => ({ RoutingWorkspace: () => null }));
vi.mock('../features/audio-session/AudioPauseControl', () => ({ AudioPauseControl: () => null }));

function fixture() {
  return new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      overruns: 0,
      underruns: 0,
    },
  });
}
function view(port: StorybookMockControlPort) {
  return (
    <MixRelayProvider>
      <I18nProvider>
        <StudioScreen controlPort={port} />
      </I18nProvider>
    </MixRelayProvider>
  );
}
beforeEach(async () => {
  commands.create = null;
  await i18n.changeLanguage('en');
});

it('opens a delegated creation command after an existing graph is loaded', async () => {
  const port = fixture();
  const mutate = vi.spyOn(port, 'mutateGraphCatalog');
  render(view(port));
  await screen.findByRole('button', { name: 'Untitled Graph' });
  expect(screen.getByRole('button', { name: 'New graph' })).toBeEnabled();
  act(() => commands.create?.());
  expect(screen.getByRole('dialog', { name: 'New graph' })).toBeVisible();
  expect(mutate).not.toHaveBeenCalled();
});

it('keeps a pre-readback creation dialog safe, then closes it when catalog ownership is known', async () => {
  const port = fixture();
  const catalog = await port.getGraphCatalog();
  const pending = Promise.withResolvers<typeof catalog>();
  vi.spyOn(port, 'getGraphCatalog').mockReturnValue(pending.promise);
  const mutate = vi.spyOn(port, 'mutateGraphCatalog');
  render(view(port));
  act(() => commands.create?.());
  expect(screen.getByRole('textbox', { name: 'Graph name' })).toHaveValue('Untitled Graph 1');
  expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
  expect(mutate).not.toHaveBeenCalled();
  await act(async () => pending.resolve(catalog));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  fireEvent.click(screen.getByRole('button', { name: 'New graph' }));
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('dialog')).toBeNull();
});
