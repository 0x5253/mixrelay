import { describe, expect, it, vi } from 'vitest';
import { protocolFixtures as fixtures } from '../../../test-support/protocolFixtures';
import { ControlPortError } from '../../domain/control/errors';
import type { RuntimeGraphDocument } from '../../domain/control/graph';
import { HttpControlPort } from './HttpControlPort';

/** Reads carry a deadline; mutations (asserted with their exact init) carry none. */
const read = { signal: expect.any(AbortSignal) };
const graph: RuntimeGraphDocument = { edges: [], nodes: [], revision: 2, schemaVersion: 2 };

it('does not fabricate a remote API for desktop-owned recorder preferences', async () => {
  const request = vi.fn<typeof fetch>();
  const port = new HttpControlPort('http://control', request);
  await expect(port.recordingSettings('graph-a', 'recorder-1')).rejects.toMatchObject({
    status: 501,
  });
  await expect(
    port.setRecordingSettings(
      'graph-a',
      'recorder-1',
      {
        folder: '/Desktop',
        sampleRate: null,
        encoding: 'pcm16',
      },
      2,
    ),
  ).rejects.toMatchObject({ status: 501 });
  expect(request).not.toHaveBeenCalled();
});

it.each(['device-a', 'device-b'])(
  'sets the rate of %s through its identity-targeted route',
  async (endpointId) => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(response(fixtures.apply(3, 'pending')));
    const port = new HttpControlPort('http://control', request);
    const format = { endpointId, expectedRevision: 2, sampleRate: 44100 };
    await expect(port.setVirtualEndpointSampleRate(format)).resolves.toMatchObject({ revision: 3 });
    expect(request).toHaveBeenCalledExactlyOnceWith(
      'http://control/api/v1/audio/virtual-endpoint/format',
      {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(format),
      },
    );
  },
);

function response(body: unknown, ok = true, status = 200): Response {
  return { json: async () => body, ok, status, headers: new Headers() } as Response;
}

function operationResponse(
  body: unknown,
  ok: boolean,
  status: number,
  operationId?: string,
  resultId?: string,
): Response {
  const headers = new Headers();
  if (operationId !== undefined) headers.set('X-MixRelay-Operation', operationId);
  if (resultId !== undefined) headers.set('X-MixRelay-Operation-Result', resultId);
  return { json: async () => body, ok, status, headers } as Response;
}

async function advanceOperationPoll(): Promise<void> {
  await Promise.resolve();
  await vi.advanceTimersByTimeAsync(100);
}

describe('HttpControlPort', () => {
  it('sends one exact-instance processor retry through the dedicated HTTP route', async () => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(
      response({
        protocolVersion: 1,
        revision: 4,
        state: 'pending',
      }),
    );
    const port = new HttpControlPort('http://control', request);
    const retry = {
      expectedCatalogRevision: 6,
      expectedInstanceId: '18446744073709551615',
      expectedRevision: 3,
      graphId: 'graph-a',
      nodeId: 'provider-a',
    };
    await expect(port.retryProcessor(retry)).resolves.toMatchObject({ revision: 4 });
    expect(request).toHaveBeenCalledExactlyOnceWith('http://control/api/v1/processors/retry', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(retry),
    });
  });

  it('targets recorder commands by encoded graph and node identity', async () => {
    const status = {
      state: 'paused',
      path: '/take.wav',
      sampleRate: 48000,
      encoding: 'pcm24',
      frames: 2048,
      error: null,
    };
    const request = vi.fn<typeof fetch>().mockResolvedValue(response(status));
    const port = new HttpControlPort('http://control', request);
    await expect(port.recorderStatus('graph/a', 'record/b')).resolves.toEqual(status);
    await expect(
      port.controlRecorder('graph/a', 'record/b', { action: 'record' }, 3),
    ).resolves.toEqual(status);
    expect(request).toHaveBeenNthCalledWith(
      1,
      'http://control/api/v1/graphs/graph%2Fa/recorders/record%2Fb',
      read,
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      'http://control/api/v1/graphs/graph%2Fa/recorders/record%2Fb',
      {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ expectedRevision: 3, request: { action: 'record' } }),
      },
    );
  });
  it('does not bind browser fetch to the adapter instance', async () => {
    const request = vi.fn(function (this: unknown) {
      expect(this).toBeUndefined();
      return Promise.resolve(response(fixtures.status()));
    });
    await expect(new HttpControlPort('http://control', request).getStatus()).resolves.toEqual(
      fixtures.status(),
    );
  });
  it('stores presentation independently with an encoded graph ID and layout revision', async () => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(response(fixtures.layout('graph/1')));
    const port = new HttpControlPort('http://control', request);
    const layout = {
      expectedLayoutRevision: 4,
      layout: { positions: {}, viewport: { x: 12, y: -3, zoom: 0.5 } },
    };
    await port.getGraphLayout('graph/1');
    await port.setGraphLayout('graph/1', layout);
    expect(request).toHaveBeenNthCalledWith(
      1,
      'http://control/api/v1/graphs/graph%2F1/layout',
      read,
    );
    expect(request).toHaveBeenNthCalledWith(2, 'http://control/api/v1/graphs/graph%2F1/layout', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(layout),
    });
  });
});

describe('HttpControlPort graph operations', () => {
  it('uses durable catalog routes with both concurrency baselines', async () => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(response(fixtures.catalog()));
    const port = new HttpControlPort('http://control', request);
    await port.getGraphCatalog();
    const mutation = {
      action: 'create' as const,
      name: 'Broadcast',
      expectedRevision: 2,
      expectedCatalogRevision: 1,
    };
    await port.mutateGraphCatalog(mutation);
    expect(request).toHaveBeenNthCalledWith(1, 'http://control/api/v1/graphs', read);
    expect(request).toHaveBeenNthCalledWith(2, 'http://control/api/v1/graphs', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(mutation),
    });
  });
  it('uses the versioned local API for status, graph reads, and graph writes', async () => {
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response(fixtures.status()))
      .mockResolvedValueOnce(response(graph))
      .mockResolvedValueOnce(response(fixtures.graphStatus(2)))
      .mockResolvedValueOnce(response(fixtures.apply(3, 'pending'), true, 202))
      .mockResolvedValueOnce(response({ processors: [], protocolVersion: 1 }))
      .mockResolvedValueOnce(response(fixtures.parameter('vst/node', 42, 0.75)))
      .mockResolvedValueOnce(response(fixtures.editor('vst/node', 'closed')))
      .mockResolvedValueOnce(response(fixtures.editor('vst/node', 'opening')));
    const port = new HttpControlPort('http://control', request);

    await expect(port.getStatus()).resolves.toMatchObject({ driverState: 'ready' });
    await expect(port.getGraph()).resolves.toEqual(graph);
    await expect(port.getGraphStatus()).resolves.toMatchObject({ activeRevision: 2 });
    await expect(port.applyGraph({ ...graph, revision: 3 })).resolves.toMatchObject({
      revision: 3,
    });
    await expect(port.getProcessors()).resolves.toMatchObject({ processors: [] });
    await expect(port.setProcessorParameter('vst/node', 42, 0.75, 3)).resolves.toMatchObject({
      value: 0.75,
    });
    await expect(port.getProcessorEditor('vst/node')).resolves.toMatchObject({ state: 'closed' });
    await expect(port.setProcessorEditor('vst/node', true, 3)).resolves.toMatchObject({
      state: 'opening',
    });
    expect(request).toHaveBeenNthCalledWith(1, 'http://control/api/v1/status', read);
    expect(request).toHaveBeenNthCalledWith(4, 'http://control/api/v1/graph', {
      body: JSON.stringify({ ...graph, revision: 3 }),
      headers: { 'content-type': 'application/json' },
      method: 'PUT',
    });
    expect(request).toHaveBeenNthCalledWith(
      6,
      'http://control/api/v1/processors/vst%2Fnode/parameters/42',
      {
        body: JSON.stringify({ value: 0.75, expectedRevision: 3 }),
        headers: { 'content-type': 'application/json' },
        method: 'PUT',
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      7,
      'http://control/api/v1/processors/vst%2Fnode/editor',
      read,
    );
    expect(request).toHaveBeenNthCalledWith(
      8,
      'http://control/api/v1/processors/vst%2Fnode/editor',
      {
        body: JSON.stringify({ open: true, expectedRevision: 3 }),
        headers: { 'content-type': 'application/json' },
        method: 'PUT',
      },
    );
  });

  it('throws the stable protocol error returned by the engine', async () => {
    const body = { code: 'graph_invalid' as const, parameters: { issues: [] } };
    const port = new HttpControlPort(
      'http://control',
      vi.fn().mockResolvedValue(response(body, false, 422)),
    );

    await expect(port.getGraph()).rejects.toEqual(new ControlPortError(422, body));
  });
});

describe('HttpControlPort pending operations', () => {
  it('polls a pending operation without replaying the mutation and acknowledges its terminal 202', async () => {
    vi.useFakeTimers();
    try {
      const operationId = 'session/a?c';
      const terminal = { protocolVersion: 1, revision: 4, state: 'pending' };
      const responses = [
        operationResponse({ operationId }, true, 202, operationId),
        operationResponse({ operationId }, true, 202, operationId),
        operationResponse(terminal, true, 202, undefined, operationId),
        operationResponse(null, true, 204),
      ];
      const request = vi.fn(function (this: unknown, ..._args: Parameters<typeof fetch>) {
        expect(this).toBeUndefined();
        return Promise.resolve(responses.shift() as Response);
      });
      const port = new HttpControlPort('http://control', request);
      const result = port.applyGraph({ ...graph, revision: 3 });

      await advanceOperationPoll();
      await advanceOperationPoll();
      await expect(result).resolves.toEqual(terminal);
      expect(request).toHaveBeenCalledTimes(4);
      expect(request).toHaveBeenNthCalledWith(1, 'http://control/api/v1/graph', {
        body: JSON.stringify({ ...graph, revision: 3 }),
        headers: { 'content-type': 'application/json' },
        method: 'PUT',
      });
      expect(request.mock.calls[1]?.[0]).toBe('http://control/api/v1/operations/session%2Fa%3Fc');
      expect(request.mock.calls[1]?.[1]).toMatchObject({ method: 'GET' });
      expect(request.mock.calls[1]?.[1]?.signal).toBeInstanceOf(AbortSignal);
      expect(request.mock.calls[3]?.[0]).toBe('http://control/api/v1/operations/session%2Fa%3Fc');
      expect(request.mock.calls[3]?.[1]).toMatchObject({ method: 'DELETE' });
      expect(request.mock.calls[3]?.[1]?.signal).toBeInstanceOf(AbortSignal);
    } finally {
      vi.useRealTimers();
    }
  });

  it('returns a terminal operation error after a matching result receipt', async () => {
    vi.useFakeTimers();
    try {
      const body = { code: 'graph_exchange_busy' as const, parameters: {} };
      const request = vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(
          operationResponse({ operationId: 'session-1' }, true, 202, 'session-1'),
        )
        .mockResolvedValueOnce(operationResponse(body, false, 409, undefined, 'session-1'))
        .mockResolvedValueOnce(operationResponse(null, true, 204));
      const port = new HttpControlPort('http://control', request);
      const result = port.applyGraph({ ...graph, revision: 3 });
      const outcome = expect(result).rejects.toEqual(new ControlPortError(409, body));

      await advanceOperationPoll();
      await outcome;
      expect(request).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('HttpControlPort polling failures', () => {
  it('propagates a poll failure without replaying the initial mutation', async () => {
    vi.useFakeTimers();
    try {
      const error = new Error('poll disconnected');
      const request = vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(
          operationResponse({ operationId: 'session-1' }, true, 202, 'session-1'),
        )
        .mockRejectedValueOnce(error);
      const port = new HttpControlPort('http://control', request);
      const result = port.applyGraph({ ...graph, revision: 3 });
      const outcome = expect(result).rejects.toBe(error);

      await advanceOperationPoll();
      await outcome;
      expect(request).toHaveBeenCalledTimes(2);
      expect(request.mock.calls[0]?.[1]).toMatchObject({ method: 'PUT' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('rejects malformed and changed operation receipts without resubmitting', async () => {
    const malformed = vi
      .fn<typeof fetch>()
      .mockResolvedValue(operationResponse({ operationId: 'different' }, true, 202, 'session-1'));
    await expect(
      new HttpControlPort('http://control', malformed).applyGraph({ ...graph, revision: 3 }),
    ).rejects.toThrow('identity changed or was invalid');
    expect(malformed).toHaveBeenCalledOnce();

    vi.useFakeTimers();
    try {
      const changed = vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(
          operationResponse({ operationId: 'session-1' }, true, 202, 'session-1'),
        )
        .mockResolvedValueOnce(
          operationResponse({ operationId: 'session-2' }, true, 202, 'session-2'),
        );
      const result = new HttpControlPort('http://control', changed).applyGraph({
        ...graph,
        revision: 3,
      });
      const outcome = expect(result).rejects.toThrow('identity changed or was invalid');
      await advanceOperationPoll();
      await outcome;
      expect(changed).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('HttpControlPort terminal receipts', () => {
  it.each([
    ['missing', undefined],
    ['wrong', 'other-session'],
  ])(
    'rejects a terminal operation without a matching result receipt (%s)',
    async (_name, resultId) => {
      vi.useFakeTimers();
      try {
        const request = vi
          .fn<typeof fetch>()
          .mockResolvedValueOnce(
            operationResponse({ operationId: 'session-1' }, true, 202, 'session-1'),
          )
          .mockResolvedValueOnce(operationResponse({ revision: 4 }, true, 202, undefined, resultId))
          .mockResolvedValueOnce(operationResponse(null, true, 204));
        const port = new HttpControlPort('http://control', request);
        const result = port.applyGraph({ ...graph, revision: 3 });
        const outcome = expect(result).rejects.toThrow('operation result could not be confirmed');

        await advanceOperationPoll();
        await outcome;
        expect(request).toHaveBeenCalledTimes(2);
      } finally {
        vi.useRealTimers();
      }
    },
  );

  it('preserves the terminal result when acknowledgement fails', async () => {
    vi.useFakeTimers();
    try {
      const terminal = { protocolVersion: 1, revision: 4, state: 'pending' };
      const request = vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(
          operationResponse({ operationId: 'session-1' }, true, 202, 'session-1'),
        )
        .mockResolvedValueOnce(operationResponse(terminal, true, 202, undefined, 'session-1'))
        .mockRejectedValueOnce(new Error('ack disconnected'));
      const port = new HttpControlPort('http://control', request);
      const result = port.applyGraph({ ...graph, revision: 3 });

      await advanceOperationPoll();
      await expect(result).resolves.toEqual(terminal);
      expect(request).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('HttpControlPort native-independent controls', () => {
  it('encodes node identities and preserves gain, power and mute revision fences', async () => {
    const controls = fixtures.nodeControls('graph/a', 7);
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response(controls))
      .mockResolvedValueOnce(response(fixtures.nodeControl(8)));
    const port = new HttpControlPort('http://control', request);
    await expect(port.getNodeControls('graph/a')).resolves.toEqual(controls);
    const mutation = { expectedRevision: 7, gainDb: -12, enabled: false, muted: true };
    await port.updateNodeControl('graph/a', 'app/b', mutation);
    expect(request.mock.calls).toEqual([
      ['http://control/api/v1/graphs/graph%2Fa/nodes/controls', read],
      [
        'http://control/api/v1/graphs/graph%2Fa/nodes/app%2Fb/control',
        {
          method: 'PUT',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(mutation),
        },
      ],
    ]);
  });

  it('returns meter snapshots and uses independent session format commands', async () => {
    const meters = { protocolVersion: 1, revision: 7, nodes: [] };
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response(meters))
      .mockResolvedValueOnce(response(fixtures.apply(8, 'pending')));
    const port = new HttpControlPort('http://control', request);
    await expect(port.getMeters()).resolves.toEqual(meters);
    const settings = { expectedRevision: 7, format: { sampleRate: 96000, blockFrames: 256 } };
    await port.reconfigureSession(settings);
    expect(request).toHaveBeenNthCalledWith(1, 'http://control/api/v1/meters', read);
    expect(request).toHaveBeenNthCalledWith(2, 'http://control/api/v1/audio/session', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(settings),
    });
  });

  it('keeps plugin rescan, native frame and editor input on dedicated routes', async () => {
    const frame = { instanceId: 'instance-1', session: 3, width: 2, height: 1, pngBase64: '' };
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response({ protocolVersion: 1, processors: [] }))
      .mockResolvedValueOnce(response(frame))
      .mockResolvedValueOnce(response(null));
    const port = new HttpControlPort('http://control', request);
    await port.rescanProcessors(['/Library/Audio/Plug-Ins/VST3']);
    await port.getProcessorEditorFrame('fx/a');
    const input = {
      instanceId: 'instance-1',
      session: 3,
      kind: 'down' as const,
      x: 12,
      y: 24,
      modifiers: 0,
    };
    await port.sendProcessorEditorInput('fx/a', input);
    expect(request).toHaveBeenNthCalledWith(1, 'http://control/api/v1/processors/scan', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ paths: ['/Library/Audio/Plug-Ins/VST3'] }),
    });
    expect(request).toHaveBeenNthCalledWith(
      2,
      'http://control/api/v1/processors/fx%2Fa/editor/frame',
      read,
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      'http://control/api/v1/processors/fx%2Fa/editor/input',
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(input),
      },
    );
  });
});

describe('HttpControlPort invalid pending receipts', () => {
  it.each([
    ['non-pending response', 'operation-1', 200],
    ['empty identity', '', 202],
    ['oversize identity', 'x'.repeat(129), 202],
  ])('rejects %s without polling or replaying the graph write', async (_name, id, status) => {
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValue(operationResponse({ operationId: id }, true, status, id));
    await expect(new HttpControlPort('http://control', request).applyGraph(graph)).rejects.toThrow(
      'invalid operation receipt',
    );
    expect(request).toHaveBeenCalledOnce();
  });

  it.each([null, 'operation-1', {}, { operationId: 1 }])(
    'rejects malformed pending receipt body %j',
    async (body) => {
      const request = vi
        .fn<typeof fetch>()
        .mockResolvedValue(operationResponse(body, true, 202, 'operation-1'));
      await expect(
        new HttpControlPort('http://control', request).applyGraph(graph),
      ).rejects.toThrow('identity changed or was invalid');
      expect(request).toHaveBeenCalledOnce();
    },
  );
});

it('uses the caller operation identity and observes its receipt without consuming it', async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValueOnce(response({ protocolVersion: 1, revision: 2, state: 'active' }))
    .mockResolvedValueOnce(operationResponse({ operationId: 'ours' }, true, 202, 'ours'))
    .mockResolvedValueOnce(response({}, false, 404))
    .mockResolvedValueOnce(operationResponse({ operationId: 'foreign' }, true, 202, 'foreign'))
    .mockResolvedValueOnce(response({ operationId: 'ours' }, true, 202));
  const port = new HttpControlPort('http://control', request);
  await port.applyGraph({ ...graph, clientOperationId: 'ours' });
  const init = request.mock.calls[0]?.[1];
  expect(new Headers(init?.headers).get('x-mixrelay-operation-request')).toBe('ours');
  expect(JSON.parse(String(init?.body))).toEqual(graph);
  await expect(port.isOperationPending('ours')).resolves.toBe(true);
  await expect(port.isOperationPending('ours')).resolves.toBe(false);
  await expect(port.isOperationPending('ours')).rejects.toThrow('identity');
  await expect(port.isOperationPending('ours')).resolves.toBe(false);
  expect(request.mock.calls.slice(1).every(([, options]) => options?.method !== 'DELETE')).toBe(
    true,
  );
});

describe('HttpControlPort response validation', () => {
  it('reports a reply that breaks its contract as an uncertain failure, not as success', async () => {
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response({ ...fixtures.status(), driverState: 'sleeping' }))
      .mockResolvedValueOnce(response({ protocolVersion: 1, revision: '4', state: 'active' }));
    const port = new HttpControlPort('http://control', request);
    await expect(port.getStatus()).rejects.toMatchObject({
      status: 502,
      body: { parameters: { path: '/api/v1/status.driverState' } },
    });
    await expect(port.applyGraph(graph)).rejects.toMatchObject({
      status: 502,
      body: { code: 'control_request_failed', parameters: { reason: 'invalid_response' } },
    });
  });

  it('validates the terminal reply of a polled operation after acknowledging it', async () => {
    vi.useFakeTimers();
    try {
      const request = vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(operationResponse({ operationId: 'op' }, true, 202, 'op'))
        .mockResolvedValueOnce(operationResponse({ revision: 4 }, true, 200, undefined, 'op'))
        .mockResolvedValueOnce(operationResponse(null, true, 204));
      const result = new HttpControlPort('http://control', request).applyGraph(graph);
      const outcome = expect(result).rejects.toMatchObject({ status: 502 });
      await advanceOperationPoll();
      await outcome;
      expect(request.mock.calls[2]?.[1]).toMatchObject({ method: 'DELETE' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps the status of a refusal with an unreadable or minimal body', async () => {
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response('<html>Bad Gateway</html>', false, 504))
      .mockResolvedValueOnce(response({ code: 'graph_invalid' }, false, 409));
    const port = new HttpControlPort('http://control', request);
    await expect(port.getGraph()).rejects.toEqual(
      new ControlPortError(504, {
        code: 'control_request_failed',
        parameters: { reason: 'invalid_error_body' },
      }),
    );
    await expect(port.getGraph()).rejects.toMatchObject({
      status: 409,
      body: { code: 'graph_invalid' },
    });
  });
});
