import {
  type StorybookControlState,
  StorybookMockControlPort,
} from '../../core/adapters/fake/StorybookMockControlPort';
import '../../test-support/integration';
import { act, render, screen } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { afterEach, assert, expect, it, vi } from 'vitest';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { advance, eventually } from '../../test-support/fakeTimerQueries';
import { RoutingWorkspace } from './RoutingWorkspace';
import { deviceActivityFixture, deviceActivitySession } from './virtualEndpointFixtures';

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function measureNodes() {
  const observers = new Map<Element, ResizeObserverCallback>();
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(private callback: ResizeObserverCallback) {}
      observe(target: Element) {
        observers.set(target, this.callback);
      }
      unobserve(target: Element) {
        observers.delete(target);
      }
      disconnect() {}
    },
  );
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(228);
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(120);
  return () =>
    act(() => {
      for (const [target, callback] of observers)
        callback(
          [
            {
              target,
              borderBoxSize: [],
              contentBoxSize: [],
              devicePixelContentBoxSize: [],
              contentRect: new DOMRect(0, 0, 228, 120),
            } as ResizeObserverEntry,
          ],
          {} as ResizeObserver,
        );
    });
}

it('presents both directions in the actual workspace without editing audio, preserves node instructions and expires failed reads', async () => {
  vi.useFakeTimers();
  await i18n.changeLanguage('en');
  const device = deviceActivitySession.virtualEndpointSettings?.devices[0];
  assert(device);
  const measure = measureNodes();
  const state: StorybookControlState = {
    graph: {
      schemaVersion: 2,
      revision: 1,
      edges: [],
      nodes: [
        {
          id: 'endpoint',
          type: 'virtual_endpoint',
          label: 'MixRelay Virtual I/O',
          control: { enabled: false, muted: true },
          config: { ...device.spec, reference: { render: true, capture: true } },
        },
      ],
    },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready' as const,
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
    deviceAppActivity: structuredClone(deviceActivityFixture),
  };
  const port = new StorybookMockControlPort(state);
  const read = vi.spyOn(port, 'getVirtualDeviceAppActivity');
  const apply = vi.spyOn(port, 'applyGraph');
  const view = render(
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <RoutingWorkspace controlPort={port} audioSession={deviceActivitySession} />
      </I18nextProvider>
    </MixRelayProvider>,
  );
  await eventually(() =>
    expect(view.container.querySelector('.react-flow__node')).toBeInTheDocument(),
  );
  await measure();
  await advance();
  await eventually(() =>
    expect(
      screen.getByRole('img', { name: /^Apps playing to MixRelay Virtual I\/O:/ }),
    ).toBeInTheDocument(),
  );
  const source = screen.getByRole('img', { name: /^Apps playing to/ });
  const sink = screen.getByRole('img', { name: /^Apps recording from/ });
  expect(source).toHaveAccessibleName(/Safari/);
  expect(source).toHaveAccessibleName(/QuickTime Player/);
  expect(sink).toHaveAccessibleName('Apps recording from MixRelay Virtual I/O: QuickTime Player');
  expect(screen.getByText('+2')).toBeVisible();
  const owner = source.closest<HTMLElement>('.react-flow__node');
  assert(owner);
  expect(owner).toHaveAccessibleDescription(/Apps playing to.*Safari/);
  expect(owner).toHaveAccessibleDescription(
    new RegExp(i18n.t('routing.a11y.nodeDescription').replace(/[.*+?^${}()|[\]\\]/g, '\\$&')),
  );
  act(() => owner.focus());
  await advance(400);
  expect(screen.getByRole('tooltip')).toHaveTextContent(/Apps playing to.*Safari/);
  act(() => owner.blur());
  read.mockRejectedValueOnce(new Error('native read failed'));
  await advance(1000);
  expect(screen.queryByRole('img', { name: /^Apps playing to/ })).toBeNull();
  expect(screen.getAllByRole('img', { name: 'App activity unavailable' })).toHaveLength(2);
  state.deviceAppActivity = {};
  await advance(1000);
  expect(
    screen.getByRole('img', { name: 'No apps playing to MixRelay Virtual I/O' }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole('img', { name: 'No apps recording from MixRelay Virtual I/O' }),
  ).toBeInTheDocument();
  expect(apply).not.toHaveBeenCalled();
  view.unmount();
});
