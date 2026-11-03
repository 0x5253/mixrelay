import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import '../test-support/integration';
import { act, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { I18nProvider, i18n } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import { App } from './App';

const menu = vi.hoisted(() => ({ open: () => {} }));
vi.mock('../core/app-info/useAppInformation', () => ({
  useAppInformation: (open: () => void) => {
    menu.open = open;
    return '1.2.3';
  },
}));

it('opens About from the native menu while the audio runtime is unavailable', async () => {
  await i18n.changeLanguage('en');
  const controlPort = new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'unavailable',
      sampleRate: null,
      bufferFrames: null,
      underruns: 0,
      overruns: 0,
    },
  });
  render(
    <MixRelayProvider>
      <I18nProvider>
        <App
          controlPort={controlPort}
          driverPort={null}
          updatesPort={null}
          runtimePort={{ connect: async () => 'unavailable', status: async () => 'unavailable' }}
        />
      </I18nProvider>
    </MixRelayProvider>,
  );
  await screen.findByRole('button', { name: i18n.t('runtimeConnection.retry') });
  expect(screen.queryByRole('textbox', { name: 'Graph name' })).not.toBeInTheDocument();
  act(() => menu.open());
  const dialog = await screen.findByRole('dialog', { name: 'About MixRelay' });
  expect(within(dialog).getByText('Version 1.2.3')).toBeVisible();
  expect(within(dialog).getByText('rawsector')).toBeVisible();
  expect(within(dialog).getByText('Source licence')).toBeVisible();
  expect(within(dialog).getByText('IBM Plex · SIL Open Font License 1.1')).toBeVisible();
});
