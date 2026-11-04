import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../core/domain/control/graph';
import '../test-support/integration';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { I18nProvider, i18n } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import { midiFixture, midiPortFixture } from '../features/midi/midiFixtures';
import { advance, eventually, find, finishFakeTimerTest } from '../test-support/fakeTimerQueries';
import { App } from './App';

const defaultMatchMedia = vi.mocked(window.matchMedia).getMockImplementation();

afterEach(() =>
  finishFakeTimerTest(advance, () => {
    if (defaultMatchMedia) vi.mocked(window.matchMedia).mockImplementation(defaultMatchMedia);
  }),
);

it.each([false, true])(
  'maps actual node controls, replaces the sidebar and restores it (narrow: %s)',
  async (narrow) => {
    const { command, user, apply } = await fixture(narrow);
    await find('group', { name: 'Monitor trim' });
    const toggle = screen.getByRole('button', { name: 'MIDI map mode' });
    expect(screen.queryByRole('button', { name: 'MIDI controls' })).not.toBeInTheDocument();
    if (narrow) {
      expect(screen.queryByRole('complementary', { name: 'Graphs' })).not.toBeInTheDocument();
      await user.click(await find('button', { name: 'Graphs' }));
    }
    const graphs = await find('complementary', { name: 'Graphs' });
    const graphButton = within(graphs).getByRole('button', {
      name: 'Untitled Graph: Delete graph',
    });
    await eventually(() => expect(graphButton).toBeEnabled());
    graphButton.focus();
    expect(graphButton).toHaveFocus();
    await user.keyboard('{Control>}m{/Control}');
    expect(screen.queryByRole('complementary', { name: 'Graphs' })).not.toBeInTheDocument();
    expect(await find('complementary', { name: 'MIDI controls' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Graphs' })).not.toBeInTheDocument();
    expect(toggle).toHaveFocus();
    expectInactiveChrome(toggle);
    const target = await find('button', { name: 'Map Monitor trim: Volume' });
    target.focus();
    await user.keyboard('{Enter}');
    expect(command).toHaveBeenCalledWith('midi_learn', {
      assignment: {
        target: { graphId: 'graph-1', nodeId: 'gain' },
        action: 'volume',
        graphName: 'Untitled Graph',
        nodeName: 'Monitor trim',
      },
    });
    await user.click(screen.getByText('Monitor trim'));
    await user.click(await find('button', { name: 'Map Monitor trim: Power' }));
    await user.keyboard('{Control>}z{/Control}{Delete}{Escape}');
    await eventually(() => expect(toggle).toHaveFocus());
    expect(command).toHaveBeenCalledWith('midi_cancel', undefined);
    expect(screen.queryByRole('complementary', { name: 'MIDI controls' })).not.toBeInTheDocument();
    expect(await find('complementary', { name: 'Graphs' })).toBeVisible();
    expect(apply).not.toHaveBeenCalled();
  },
);

it.each([false, true])(
  'moves focus from unavailable app chrome into MIDI mode (narrow: %s)',
  async (narrow) => {
    const { user } = await fixture(narrow, []);
    await find('region', { name: 'MixRelay virtual I/O routing graph' });
    await expectMapEntryMovesFocus(user);
  },
);

async function fixture(
  narrow = false,
  nodes: RuntimeGraphDocument['nodes'] = [
    { id: 'gain', type: 'gain', label: 'Monitor trim', config: { channels: 2, gainDb: 0 } },
  ],
) {
  vi.useFakeTimers();
  installGeometry();
  if (narrow) {
    const media = window.matchMedia('(max-width: 720px)');
    vi.spyOn(window, 'matchMedia').mockImplementation((query) => ({
      ...media,
      media: query,
      matches: query === '(max-width: 720px)',
    }));
  }
  await i18n.changeLanguage('en');
  const graph: RuntimeGraphDocument = {
    revision: 1,
    schemaVersion: 2,
    edges: [],
    nodes,
  };
  const port = new StorybookMockControlPort({
    graph,
    graphStatus: { activeRevision: 1, submittedRevision: 1, protocolVersion: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      bufferFrames: 128,
      sampleRate: 48000,
      overruns: 0,
      underruns: 0,
    },
  });
  const midi = midiPortFixture({ ...midiFixture, bindings: [] });
  const command = vi.spyOn(midi, 'command');
  // Spy before mounting so writes during the first render are counted too.
  const apply = vi.spyOn(port, 'applyGraph');
  render(
    <MixRelayProvider>
      <I18nProvider>
        <App
          controlPort={port}
          midiPort={{ command }}
          driverPort={null}
          runtimePort={null}
          updatesPort={null}
        />
      </I18nProvider>
    </MixRelayProvider>,
  );
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  return { port, command, user, apply };
}

function installGeometry() {
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(1024);
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(768);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(
    new DOMRect(0, 0, 1024, 768),
  );
  class MeasuredObserver implements ResizeObserver {
    constructor(private callback: ResizeObserverCallback) {}
    observe(target: Element) {
      const size = [{ inlineSize: 1024, blockSize: 768 }];
      this.callback(
        [
          {
            target,
            contentRect: new DOMRect(0, 0, 1024, 768),
            borderBoxSize: size,
            contentBoxSize: size,
            devicePixelContentBoxSize: size,
          },
        ],
        this,
      );
    }
    disconnect() {}
    unobserve() {}
  }
  vi.stubGlobal('ResizeObserver', MeasuredObserver);
}

function expectInactiveChrome(toggle: HTMLElement) {
  const theme = screen.getByRole('button', { name: /Appearance:/ });
  const graphName = screen.getByRole('textbox', { name: i18n.t('studio.graphName') });
  expect(theme.closest('[inert]')).not.toBeNull();
  expect(toggle.closest('header')).not.toBe(theme.closest('header'));
  expect(toggle.closest('header')?.querySelector('input')).toBe(graphName);
  expect(graphName).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Diagnostics' }).closest('[inert]')).not.toBeNull();
}

async function expectMapEntryMovesFocus(user: ReturnType<typeof userEvent.setup>) {
  const toggle = screen.getByRole('button', { name: 'MIDI map mode' });
  for (const control of [
    () => screen.getByRole('button', { name: /Appearance:/ }),
    () => screen.getByRole('textbox', { name: i18n.t('studio.graphName') }),
    () => screen.getByRole('button', { name: 'Diagnostics' }),
  ]) {
    control().focus();
    await user.keyboard('{Control>}m{/Control}');
    await eventually(() => expect(toggle).toHaveFocus());
    await user.keyboard('{Escape}');
  }
}
