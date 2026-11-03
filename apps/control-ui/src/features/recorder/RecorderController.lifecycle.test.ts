import { afterEach, describe, expect, it, vi } from 'vitest';
import { timerScheduler } from '../../core/adapters/browser/timers';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { ControlPortError } from '../../core/domain/control/errors';
import type { RecorderSettings, RecorderStatus } from '../../core/domain/control/recorder';
import type { RecorderPort } from '../../core/ports/control/RecorderPort';
import { RecorderController } from './application/RecorderController';
import { recorderError } from './domain/recording';

/** Gives the fake engine the optional selectRecordingFolder capability. */
function offerFolderChooser(port: RecorderPort, value: RecorderPort['selectRecordingFolder']) {
  port.selectRecordingFolder = value;
}

const stopped: RecorderStatus = {
  state: 'stopped',
  path: null,
  sampleRate: null,
  encoding: null,
  frames: 0,
  error: null,
};
function fixture() {
  const port = new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 256,
      underruns: 0,
      overruns: 0,
    },
  });
  const read = vi.spyOn(port, 'recorderStatus').mockResolvedValue(stopped);
  const write = vi.spyOn(port, 'controlRecorder').mockResolvedValue(stopped);
  const save = vi.spyOn(port, 'setRecordingSettings');
  const controller = new RecorderController(port, 'graph-a', 'recorder-1', timerScheduler);
  return { port, read, write, save, controller };
}
async function flush() {
  for (let i = 0; i < 12; i += 1) await Promise.resolve();
}
afterEach(() => vi.useRealTimers());

it('does not double-start the owner and notifies only subscribed listeners', async () => {
  vi.useFakeTimers();
  const f = fixture();
  const listener = vi.fn();
  const unsubscribe = f.controller.subscribe(listener);
  f.controller.start();
  f.controller.start();
  await flush();
  expect(f.read).toHaveBeenCalledTimes(1);
  expect(listener).toHaveBeenCalledTimes(3);
  unsubscribe();
  await vi.advanceTimersByTimeAsync(1000);
  expect(listener).toHaveBeenCalledTimes(3);
  f.controller.dispose();
});

it('does not publish or keep polling after a subscriber disposes during a settings notification', async () => {
  vi.useFakeTimers();
  const f = fixture();
  f.controller.subscribe(() => {
    if (f.controller.getSnapshot().settings) f.controller.dispose();
  });
  f.controller.start();
  await flush();
  expect(f.controller.getSnapshot().status).toBeNull();
  expect(vi.getTimerCount()).toBe(0);
});

it('leaves folder choice inert when the native provider has no picker', async () => {
  const f = fixture();
  f.controller.start();
  await flush();
  f.controller.chooseFolder('Folder', 1);
  expect(f.controller.getSnapshot().busy).toBe(false);
  expect(f.save).not.toHaveBeenCalled();
  f.controller.dispose();
});

describe('queued recorder owner disposal', () => {
  it('does not send a queued command after its owner disappears', async () => {
    const f = fixture();
    f.controller.start();
    await flush();
    f.controller.command({ action: 'stop' }, 3);
    f.controller.dispose();
    await flush();
    expect(f.write).not.toHaveBeenCalled();
    f.controller.command({ action: 'stop' }, 3);
    expect(f.write).not.toHaveBeenCalled();
  });

  it('does not send a queued settings edit after disposal', async () => {
    const f = fixture();
    f.controller.start();
    await flush();
    f.controller.saveSettings({ encoding: 'pcm24' }, 3);
    f.controller.dispose();
    await flush();
    expect(f.save).not.toHaveBeenCalled();
  });

  it('skips a queued read while a stop command owns the recorder', async () => {
    const f = fixture();
    f.controller.start();
    f.controller.command({ action: 'stop' }, 3);
    await flush();
    expect(f.read).not.toHaveBeenCalled();
    expect(f.write).toHaveBeenCalledExactlyOnceWith('graph-a', 'recorder-1', { action: 'stop' }, 3);
    f.controller.dispose();
  });
});

describe('late recorder command responses', () => {
  it.each(['accept', 'reject'] as const)(
    'ignores a late %s after command ownership ends',
    async (outcome) => {
      vi.useFakeTimers();
      const f = fixture();
      f.controller.start();
      await flush();
      const pending = Promise.withResolvers<RecorderStatus>();
      f.write.mockReturnValueOnce(pending.promise);
      f.controller.command({ action: 'stop' }, 3);
      await flush();
      const before = f.controller.getSnapshot();
      f.controller.dispose();
      if (outcome === 'accept') pending.resolve({ ...stopped, state: 'recording' });
      else pending.reject(new Error('old command failed'));
      await flush();
      expect(f.controller.getSnapshot()).toBe(before);
      expect(vi.getTimerCount()).toBe(0);
    },
  );

  it('keeps the command failure when a subsequent status read also fails', async () => {
    vi.useFakeTimers();
    const f = fixture();
    f.controller.start();
    await flush();
    f.write.mockRejectedValueOnce(new Error('command fault'));
    f.controller.command({ action: 'stop' }, 3);
    await flush();
    f.read.mockRejectedValueOnce(new Error('read fault'));
    await vi.advanceTimersByTimeAsync(1000);
    expect(f.controller.getSnapshot().error).toBe('command fault');
    expect(f.controller.getSnapshot().status).toBe(stopped);
    f.controller.dispose();
  });
});

describe('settings errors and active take permissions', () => {
  it.each(['recording', 'paused'] as const)(
    'does not change canonical settings during a %s take',
    async (state) => {
      const f = fixture();
      f.read.mockResolvedValue({ ...stopped, state });
      f.controller.start();
      await flush();
      f.controller.saveSettings({ encoding: 'pcm24' }, 3);
      expect(f.save).not.toHaveBeenCalled();
      expect(f.controller.getSnapshot().busy).toBe(false);
      f.controller.dispose();
    },
  );

  it('allows paused recording to resume even when canonical settings cannot be read', async () => {
    const f = fixture();
    f.read.mockResolvedValue({ ...stopped, state: 'paused' });
    vi.spyOn(f.port, 'recordingSettings').mockRejectedValue(new Error('settings offline'));
    f.controller.start();
    await flush();
    expect(f.controller.getSnapshot().settings).toBeNull();
    f.controller.command({ action: 'record' }, 3);
    await flush();
    expect(f.write).toHaveBeenCalledExactlyOnceWith(
      'graph-a',
      'recorder-1',
      { action: 'record' },
      3,
    );
    f.controller.dispose();
  });

  it('preserves settings when a folder picker fails and ignores its later rejection after disposal', async () => {
    const f = fixture();
    const pending = Promise.withResolvers<string | null>();
    const choose = vi
      .fn()
      .mockRejectedValueOnce(new Error('picker fault'))
      .mockReturnValueOnce(pending.promise);
    offerFolderChooser(f.port, choose);
    f.controller.start();
    await flush();
    f.controller.chooseFolder('Folder', 3);
    await flush();
    expect(f.controller.getSnapshot().settingsError).toBe('picker fault');
    expect(f.controller.getSnapshot().settings?.folder).toBe('/Desktop');
    f.controller.chooseFolder('Folder', 3);
    await flush();
    const before = f.controller.getSnapshot();
    f.controller.dispose();
    pending.reject(new Error('old picker failed'));
    await flush();
    expect(f.controller.getSnapshot()).toBe(before);
    expect(f.save).not.toHaveBeenCalled();
  });

  it('ignores an old settings save rejection after disposal', async () => {
    const f = fixture();
    f.controller.start();
    await flush();
    const pending = Promise.withResolvers<RecorderSettings>();
    f.save.mockReturnValueOnce(pending.promise);
    f.controller.saveSettings({ encoding: 'pcm24' }, 3);
    await flush();
    const before = f.controller.getSnapshot();
    f.controller.dispose();
    pending.reject(new Error('old save failed'));
    await flush();
    expect(f.controller.getSnapshot()).toBe(before);
  });
});

it.each([null, 'disk failure', 42, { problem: true }])(
  'reports arbitrary rejection values %j without inventing a server reason',
  (error) => {
    expect(recorderError(error)).toBe(String(error));
  },
);
it.each([null, 'plain parameter', { reason: null }, { reason: 42 }])(
  'keeps structured server errors with unusable reason %j',
  (parameters) => {
    const error = new ControlPortError(422, { code: 'recorder_failed', parameters });
    expect(recorderError(error)).toBe(error.message);
  },
);
