import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ControlPortError } from '../../core/domain/control/errors';
import type { ProcessorParameterUpdateResponse } from '../../core/domain/control/processors';

import { useProcessorParameterPublisher } from './useProcessorParameterPublisher';

const response: ProcessorParameterUpdateResponse = {
  nodeId: 'p',
  parameterId: 1,
  protocolVersion: 1,
  value: 0.5,
};
const update = { expectedRevision: 1, nodeId: 'p', parameterId: 1, value: 0.5 };

afterEach(cleanup);

describe('processor parameter durable acknowledgements', () => {
  it('orders durable acknowledgements across controls without losing coalesced values', async () => {
    const first = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const second = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const port = {
      setProcessorParameter: vi
        .fn()
        .mockReturnValueOnce(first.promise)
        .mockReturnValueOnce(second.promise)
        .mockResolvedValue(response),
    };
    const warning = vi.fn();
    const { result } = renderHook(() => useProcessorParameterPublisher(port, warning));
    act(() => {
      result.current.publish(update);
      result.current.publish({ ...update, parameterId: 2, value: 0.2 });
      result.current.publish({ ...update, parameterId: 2, value: 0.3 });
      result.current.publish({ ...update, value: 0.9 });
    });
    expect(port.setProcessorParameter).toHaveBeenCalledTimes(1);
    await act(async () => first.resolve({ ...response, persistenceError: 'disk full' }));
    expect(warning).toHaveBeenLastCalledWith('disk full');
    expect(port.setProcessorParameter).toHaveBeenLastCalledWith('p', 2, 0.3, 1);
    expect(result.current.busy).toBe(true);
    await act(async () => second.resolve({ ...response, parameterId: 2 }));
    expect(port.setProcessorParameter.mock.calls).toEqual([
      ['p', 1, 0.5, 1],
      ['p', 2, 0.3, 1],
      ['p', 1, 0.9, 1],
    ]);
    expect(warning.mock.calls).toEqual([['disk full'], [null], [null]]);
    expect(result.current.busy).toBe(false);
  });

  it('does not clear a storage warning on rejected or stale acknowledgements', async () => {
    const old = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const oldPort = { setProcessorParameter: vi.fn().mockReturnValue(old.promise) };
    const nextPort = { setProcessorParameter: vi.fn().mockRejectedValue(new Error('offline')) };
    const warning = vi.fn();
    const { result, rerender } = renderHook(
      ({ port }) => useProcessorParameterPublisher(port, warning),
      { initialProps: { port: oldPort } },
    );
    act(() => result.current.publish(update));
    rerender({ port: nextPort });
    await act(async () => result.current.publish(update));
    await act(async () => old.resolve(response));
    expect(warning).not.toHaveBeenCalled();
    expect(result.current.busy).toBe(false);
  });
});

describe('processor parameter publishing busy state', () => {
  it('stays busy while each control drains its latest queued value', async () => {
    const first = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const last = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const port = {
      setProcessorParameter: vi
        .fn()
        .mockReturnValueOnce(first.promise)
        .mockReturnValue(last.promise),
    };
    const { result } = renderHook(() => useProcessorParameterPublisher(port));
    act(() => {
      result.current.publish(update);
      result.current.publish({ ...update, value: 0.6 });
      result.current.publish({ ...update, value: 0.7 });
    });
    expect(result.current.busy).toBe(true);
    expect(port.setProcessorParameter).toHaveBeenCalledTimes(1);
    await act(async () => first.resolve(response));
    expect(result.current.busy).toBe(true);
    expect(port.setProcessorParameter).toHaveBeenLastCalledWith('p', 1, 0.7, 1);
    await act(async () => last.resolve(response));
    expect(result.current.busy).toBe(false);
  });

  it('clears busy on failure and permits a fresh retry', async () => {
    const port = {
      setProcessorParameter: vi
        .fn()
        .mockRejectedValueOnce(new Error('lost'))
        .mockResolvedValue(response),
    };
    const { result } = renderHook(() => useProcessorParameterPublisher(port));
    await act(async () => result.current.publish(update));
    expect(result.current.busy).toBe(false);
    await act(async () => result.current.publish(update));
    expect(result.current.busy).toBe(false);
    expect(port.setProcessorParameter).toHaveBeenCalledTimes(2);
  });

  it('cannot drain a replacement control port with old replies or publish after unmount', async () => {
    const old = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const next = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const oldPort = { setProcessorParameter: vi.fn().mockReturnValue(old.promise) };
    const nextPort = { setProcessorParameter: vi.fn().mockReturnValue(next.promise) };
    const { result, rerender, unmount } = renderHook(
      ({ port }) => useProcessorParameterPublisher(port),
      {
        initialProps: { port: oldPort },
      },
    );
    act(() => result.current.publish(update));
    rerender({ port: nextPort });
    act(() => result.current.publish(update));
    await act(async () => old.reject(new Error('old IPC rejected')));
    expect(result.current.busy).toBe(true);
    unmount();
    act(() => result.current.publish(update));
    await act(async () => next.resolve(response));
    expect(nextPort.setProcessorParameter).toHaveBeenCalledTimes(1);
  });
});

describe('processor parameter rejected intent ownership', () => {
  it('keeps the newer same-key request after an in-flight rejection', async () => {
    const first = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const newer = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const port = {
      setProcessorParameter: vi
        .fn()
        .mockReturnValueOnce(first.promise)
        .mockReturnValueOnce(newer.promise),
    };
    const { result } = renderHook(() => useProcessorParameterPublisher(port));
    act(() => {
      result.current.publish(update);
      result.current.publish({ ...update, value: 0.9 });
    });
    await act(async () =>
      first.reject(new ControlPortError(503, { code: 'processor_unavailable', parameters: {} })),
    );
    expect(port.setProcessorParameter.mock.calls).toEqual([
      ['p', 1, 0.5, 1],
      ['p', 1, 0.9, 1],
    ]);
    expect(result.current.failures).toMatchObject([{ update, code: 'processor_unavailable' }]);
    expect(result.current.busy).toBe(true);
    await act(async () => newer.resolve({ ...response, value: 0.9 }));
    expect(result.current.failures).toEqual([]);
    await expect(result.current.whenIdle()).resolves.toBeUndefined();
  });

  it('blocks a subsequent graph operation until the preserved intent is explicitly retried', async () => {
    const port = {
      setProcessorParameter: vi
        .fn()
        .mockRejectedValueOnce(new Error('offline'))
        .mockResolvedValue(response),
    };
    const { result } = renderHook(() => useProcessorParameterPublisher(port));
    await act(async () => result.current.publish(update));
    const applyGraph = vi.fn();
    await expect(result.current.whenIdle().then(applyGraph)).rejects.toMatchObject({
      body: { code: 'control_request_failed' },
    });
    expect(applyGraph).not.toHaveBeenCalled();
    expect(result.current.failures).toMatchObject([{ update, code: 'control_request_failed' }]);
    await act(async () => result.current.retryFailed());
    expect(port.setProcessorParameter.mock.calls).toEqual([
      ['p', 1, 0.5, 1],
      ['p', 1, 0.5, 1],
    ]);
    await expect(result.current.whenIdle()).resolves.toBeUndefined();
    expect(result.current.failures).toEqual([]);
  });

  it('does not erase another control failure when a different control succeeds', async () => {
    const port = {
      setProcessorParameter: vi
        .fn()
        .mockRejectedValueOnce(new Error('offline'))
        .mockResolvedValue(response),
    };
    const { result } = renderHook(() => useProcessorParameterPublisher(port));
    await act(async () => result.current.publish(update));
    await act(async () => result.current.publish({ ...update, parameterId: 2, value: 0.8 }));
    expect(result.current.failures).toMatchObject([{ update }]);
    await expect(result.current.whenIdle()).rejects.toBeInstanceOf(ControlPortError);
    act(() => result.current.discardFailures());
    expect(result.current.failures).toEqual([]);
    await expect(result.current.whenIdle()).resolves.toBeUndefined();
    expect(port.setProcessorParameter).toHaveBeenCalledTimes(2);
  });
});

describe('processor parameter retry and session ownership', () => {
  it('does not replace a newer in-flight edit with an older retry value', async () => {
    const latest = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const port = {
      setProcessorParameter: vi
        .fn()
        .mockRejectedValueOnce(new Error('offline'))
        .mockReturnValueOnce(latest.promise),
    };
    const { result } = renderHook(() => useProcessorParameterPublisher(port));
    await act(async () => result.current.publish(update));
    act(() => {
      result.current.publish({ ...update, value: 0.8 });
      result.current.retryFailed();
    });
    await act(async () => latest.resolve({ ...response, value: 0.8 }));
    expect(port.setProcessorParameter.mock.calls).toEqual([
      ['p', 1, 0.5, 1],
      ['p', 1, 0.8, 1],
    ]);
    expect(result.current.failures).toEqual([]);
  });

  it('retains only the latest failed generation for retry', async () => {
    const port = {
      setProcessorParameter: vi
        .fn()
        .mockRejectedValueOnce(new Error('one'))
        .mockRejectedValueOnce(new Error('two'))
        .mockResolvedValue(response),
    };
    const { result } = renderHook(() => useProcessorParameterPublisher(port));
    await act(async () => result.current.publish(update));
    const firstId = result.current.failures[0]?.requestId;
    await act(async () => result.current.publish({ ...update, value: 0.7 }));
    expect(result.current.failures).toHaveLength(1);
    expect(result.current.failures[0]?.requestId).toBeGreaterThan(firstId ?? 0);
    await act(async () => result.current.retryFailed());
    expect(port.setProcessorParameter).toHaveBeenLastCalledWith('p', 1, 0.7, 1);
  });

  it('isolates new-port failures from old-port success and old-port retry keys', async () => {
    const old = Promise.withResolvers<ProcessorParameterUpdateResponse>();
    const oldPort = { setProcessorParameter: vi.fn().mockReturnValue(old.promise) };
    const nextPort = { setProcessorParameter: vi.fn().mockRejectedValue(new Error('new failure')) };
    const { result, rerender } = renderHook(({ port }) => useProcessorParameterPublisher(port), {
      initialProps: { port: oldPort },
    });
    act(() => result.current.publish(update));
    rerender({ port: nextPort });
    await act(async () => result.current.publish({ ...update, value: 0.2 }));
    await act(async () => old.resolve(response));
    expect(result.current.failures).toMatchObject([{ update: { ...update, value: 0.2 } }]);
    await expect(result.current.whenIdle()).rejects.toBeInstanceOf(ControlPortError);
  });
});

it('makes retained publication callbacks inert after unmount and resolves idle immediately', async () => {
  const port = { setProcessorParameter: vi.fn().mockResolvedValue(response) };
  const { result, unmount } = renderHook(() => useProcessorParameterPublisher(port));
  const actions = result.current;
  unmount();
  act(() => {
    actions.publish(update);
    actions.retryFailed();
    actions.discardFailures();
  });
  await expect(actions.whenIdle()).resolves.toBeUndefined();
  expect(port.setProcessorParameter).not.toHaveBeenCalled();
});
