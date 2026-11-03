import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import '../test-support/integration';
import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import axe from 'axe-core';
import { expect, it } from 'vitest';
import { I18nProvider, i18n } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import { midiBinding, midiFixture, midiPortFixture } from '../features/midi/midiFixtures';
import { App } from './App';

it('map mode exposes named grouped mappings and a single title focus stop while application chrome is inert', async () => {
  await i18n.changeLanguage('en');
  const controlPort = new StorybookMockControlPort({
    graph: { revision: 1, schemaVersion: 2, nodes: [], edges: [] },
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
  const midiPort = midiPortFixture({
    ...midiFixture,
    mapMode: true,
    bindings: [
      midiBinding,
      {
        ...midiBinding,
        nodeName: 'Programme output',
        target: { graphId: 'main', nodeId: 'other' },
      },
    ],
  });
  render(
    <MixRelayProvider>
      <I18nProvider>
        <App
          controlPort={controlPort}
          midiPort={midiPort}
          driverPort={null}
          runtimePort={null}
          updatesPort={null}
        />
      </I18nProvider>
    </MixRelayProvider>,
  );
  await act(async () => {});
  const panel = screen.getByRole('complementary', { name: 'MIDI controls' });
  expect(within(panel).getAllByRole('heading', { level: 3 })).toHaveLength(1);
  expect(screen.getByRole('button', { name: /Appearance:/ }).closest('[inert]')).not.toBeNull();
  expect(screen.getByRole('button', { name: 'Diagnostics' }).closest('[inert]')).not.toBeNull();
  expect(screen.getByRole('textbox', { name: i18n.t('studio.graphName') })).toBeDisabled();
  const order = [
    'Select Programme / Microphone · Volume',
    'Remove MIDI control for Programme / Microphone',
    'Select Programme / Programme output · Volume',
    'Remove MIDI control for Programme / Programme output',
    'Finish mapping',
    'MIDI map mode',
  ];
  const user = userEvent.setup();
  screen.getByRole('button', { name: order[0] }).focus();
  for (const name of order.slice(1)) {
    await user.tab();
    expect(screen.getByRole('button', { name })).toHaveFocus();
  }
  for (const name of order.slice(0, -1).reverse()) {
    await user.tab({ shift: true });
    expect(screen.getByRole('button', { name })).toHaveFocus();
  }
  const result = await axe.run(document.body, {
    rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
  });
  expect(
    result.violations.map((violation) => ({
      rule: violation.id,
      nodes: violation.nodes.map((node) => node.html),
    })),
  ).toEqual([]);
});
