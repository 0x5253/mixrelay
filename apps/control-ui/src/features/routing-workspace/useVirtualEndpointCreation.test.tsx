import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { I18nextProvider } from 'react-i18next';
import { afterEach, expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { VirtualEndpointSpec } from '../../core/domain/control/status';
import { i18n } from '../../core/i18n';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './model';
import { useVirtualEndpointCreation } from './useVirtualEndpointCreation';
import { virtualPolicy, virtualSpec } from './virtualEndpointFixtures';
import { virtualEndpointItemId } from './virtualEndpointMenu';

const graph: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
afterEach(() => vi.restoreAllMocks());
function owner() {
  return new StorybookMockControlPort({
    graph,
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 96000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  });
}
function Harness({
  port,
  enabled = true,
  createDevice = async (spec) => spec,
}: {
  port: ControlPort;
  enabled?: boolean;
  createDevice?: (spec: VirtualEndpointSpec) => Promise<VirtualEndpointSpec | null>;
}) {
  const [model, setModel] = useState(() => runtimeGraphToAudioGraph(graph));
  const action = useVirtualEndpointCreation({
    owner: port,
    enabled,
    model,
    settings: virtualPolicy,
    createDevice,
    updateModel: setModel,
  });
  return (
    <>
      <button type="button" onClick={() => action.tryAdd('virtual-new-input', { x: 12, y: 34 })}>
        New
      </button>
      <button
        type="button"
        onClick={() =>
          action.tryAdd(virtualEndpointItemId('input', virtualSpec.id), { x: 1, y: 2 })
        }
      >
        Share input
      </button>
      <button
        type="button"
        onClick={() =>
          action.tryAdd(virtualEndpointItemId('output', virtualSpec.id), { x: 3, y: 4 })
        }
      >
        Share output
      </button>
      <output data-testid="saved">{JSON.stringify(audioGraphToRuntimeGraph(model, graph))}</output>
      {action.dialog}
    </>
  );
}
function view(
  port: ControlPort,
  enabled = true,
  createDevice?: (spec: VirtualEndpointSpec) => Promise<VirtualEndpointSpec | null>,
) {
  return (
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        <Harness port={port} enabled={enabled} createDevice={createDevice} />
      </I18nextProvider>
    </MixRelayProvider>
  );
}

it('allocates one identity only on named confirmation and retains the device template rate', async () => {
  await i18n.changeLanguage('en');
  const uuid = vi
    .spyOn(crypto, 'randomUUID')
    .mockReturnValue('bf6aa0e2-8e8e-4672-b7a8-22e481606b54');
  render(view(owner()));
  fireEvent.click(screen.getByRole('button', { name: 'New' }));
  const name = await screen.findByRole('textbox', { name: 'Device name' });
  expect(uuid).not.toHaveBeenCalled();
  fireEvent.change(name, { target: { value: '  Monitor return  ' } });
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  expect(uuid).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(screen.getByTestId('saved')).toHaveTextContent('Monitor return'));
  expect(screen.getByTestId('saved')).toHaveTextContent('"defaultSampleRate":44100');
  expect(screen.getByTestId('saved')).toHaveTextContent(
    '"reference":{"render":true,"capture":false}',
  );
});

it('waits for owner creation readback and ignores a late result after cancellation', async () => {
  await i18n.changeLanguage('en');
  let resolve!: (spec: VirtualEndpointSpec) => void;
  const createDevice = vi.fn(
    (_spec: VirtualEndpointSpec) =>
      new Promise<VirtualEndpointSpec>((done) => {
        resolve = done;
      }),
  );
  render(view(owner(), true, createDevice));
  fireEvent.click(screen.getByRole('button', { name: 'New' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Create' }));
  expect(createDevice).toHaveBeenCalledOnce();
  expect(screen.getByTestId('saved')).toHaveTextContent('"nodes":[]');
  fireEvent.keyDown(screen.getByRole('textbox', { name: 'Device name' }), {
    key: 'Escape',
    code: 'Escape',
  });
  const submitted = createDevice.mock.calls[0]?.[0];
  if (!submitted) throw new Error('Expected a submitted virtual device');
  await act(async () => resolve({ ...submitted, displayName: 'Actual readback' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(screen.getByTestId('saved')).toHaveTextContent('"nodes":[]');
});

it('shares a canonical identity on both sides without allocating or renaming a device', () => {
  const uuid = vi.spyOn(crypto, 'randomUUID');
  render(view(owner()));
  fireEvent.click(screen.getByRole('button', { name: 'Share input' }));
  fireEvent.click(screen.getByRole('button', { name: 'Share output' }));
  expect(uuid).toHaveBeenCalledTimes(1);
  const saved = JSON.parse(screen.getByTestId('saved').textContent ?? '{}') as RuntimeGraphDocument;
  expect(saved.nodes).toHaveLength(1);
  expect(saved.nodes[0]?.id).toMatch(/^virtual-[0-9a-f-]{36}$/);
  expect(saved.nodes[0]?.config).toMatchObject({ id: virtualSpec.id });
  expect(screen.getByTestId('saved')).toHaveTextContent(virtualSpec.id);
  expect(screen.getByTestId('saved')).toHaveTextContent(
    '"reference":{"render":true,"capture":true}',
  );
});

it('invalidates a creation dialog when its engine owner changes', async () => {
  const uuid = vi.spyOn(crypto, 'randomUUID');
  const mounted = render(view(owner()));
  fireEvent.click(screen.getByRole('button', { name: 'New' }));
  await screen.findByRole('dialog');
  mounted.rerender(view(owner()));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(uuid).not.toHaveBeenCalled();
  expect(screen.getByTestId('saved')).toHaveTextContent('"nodes":[]');
});

it('does not create or share devices while the mutation gate is closed', () => {
  render(view(owner(), false));
  fireEvent.click(screen.getByRole('button', { name: 'New' }));
  fireEvent.click(screen.getByRole('button', { name: 'Share input' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByTestId('saved')).toHaveTextContent('"nodes":[]');
});
