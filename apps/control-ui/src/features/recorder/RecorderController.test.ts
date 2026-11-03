import { afterEach, describe, expect, it, vi } from 'vitest';
import { timerScheduler } from '../../core/adapters/browser/timers';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { ControlPortError } from '../../core/domain/control/errors';
import type { RecorderSettings, RecorderStatus } from '../../core/domain/control/recorder';
import { RecorderController } from './application/RecorderController';
import { recordingDuration } from './domain/recording';

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
  const write = vi
    .spyOn(port, 'controlRecorder')
    .mockResolvedValue({ ...stopped, state: 'recording' });
  const controller = new RecorderController(port, 'graph-a', 'recorder-1', timerScheduler);
  return { controller, read, write, port };
}
const flush = async () => {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
};

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

it('keeps Storybook shared settings isolated by graph and recorder without aliasing objects', async () => {
  const { port } = fixture();
  await port.setRecordingSettings(
    'graph-a',
    'recorder-1',
    { folder: '/One', sampleRate: 44100, encoding: 'pcm24' },
    1,
  );
  expect((await port.recordingSettings('graph-b', 'recorder-1')).folder).toBe('/Desktop');
  expect((await port.recordingSettings('graph-a', 'recorder-2')).folder).toBe('/Desktop');
  const copy = await port.recordingSettings('graph-a', 'recorder-1');
  copy.folder = '/mutated';
  expect((await port.recordingSettings('graph-a', 'recorder-1')).folder).toBe('/One');
});

it('loads external settings on the same serialized status polling owner', async () => {
  vi.useFakeTimers();
  const { controller, port } = fixture();
  controller.start();
  await flush();
  const settings: RecorderSettings = { folder: '/Deck', sampleRate: 96000, encoding: 'pcm24' };
  await port.setRecordingSettings('graph-a', 'recorder-1', settings, 1);
  await vi.advanceTimersByTimeAsync(1000);
  expect(controller.getSnapshot().settings).toEqual(settings);
  controller.dispose();
});

it('blocks Record and other settings writes until the canonical save acknowledges', async () => {
  vi.useFakeTimers();
  const { controller, port, write } = fixture();
  controller.start();
  await flush();
  const pending = deferred<RecorderSettings>();
  const save = vi.spyOn(port, 'setRecordingSettings').mockReturnValueOnce(pending.promise);
  controller.saveSettings({ sampleRate: 96000 }, 1);
  controller.saveSettings({ encoding: 'float32' }, 1);
  controller.command({ action: 'record' }, 1);
  await flush();
  expect(controller.getSnapshot().busy).toBe(true);
  expect(save).toHaveBeenCalledExactlyOnceWith(
    'graph-a',
    'recorder-1',
    {
      folder: '/Desktop',
      sampleRate: 96000,
      encoding: 'pcm16',
    },
    1,
  );
  expect(write).not.toHaveBeenCalled();
  pending.resolve({ folder: '/Desktop', sampleRate: 96000, encoding: 'pcm16' });
  await flush();
  controller.command({ action: 'record' }, 1);
  await flush();
  expect(write).toHaveBeenCalledExactlyOnceWith('graph-a', 'recorder-1', { action: 'record' }, 1);
  controller.dispose();
});

it('blocks Record after failed save until a fresh canonical load or explicit save succeeds', async () => {
  vi.useFakeTimers();
  const { controller, port, write } = fixture();
  controller.start();
  await flush();
  const save = vi.spyOn(port, 'setRecordingSettings').mockRejectedValueOnce(new Error('Disk full'));
  controller.saveSettings({ encoding: 'pcm24' }, 1);
  await flush();
  expect(controller.getSnapshot().settingsError).toBe('Disk full');
  controller.command({ action: 'record' }, 1);
  await vi.advanceTimersByTimeAsync(1000);
  expect(write).not.toHaveBeenCalled();
  expect(save).toHaveBeenCalledTimes(1);
  expect(controller.getSnapshot().settingsError).toBeNull();
  expect(controller.getSnapshot().settings?.encoding).toBe('pcm16');
  controller.command({ action: 'record' }, 1);
  await flush();
  expect(write).toHaveBeenCalledTimes(1);
  controller.command({ action: 'stop' }, 1);
  write.mockResolvedValue(stopped);
  await flush();
  controller.saveSettings({ encoding: 'pcm24' }, 1);
  await flush();
  expect(controller.getSnapshot().settingsError).toBeNull();
  expect(controller.getSnapshot().settings?.encoding).toBe('pcm24');
  controller.dispose();
});

it('preserves Stop and Pause when reading settings fails during an actual recording', async () => {
  vi.useFakeTimers();
  const { controller, port, read, write } = fixture();
  read.mockResolvedValue({ ...stopped, state: 'recording', sampleRate: 48000, encoding: 'pcm24' });
  vi.spyOn(port, 'recordingSettings').mockRejectedValue(new Error('Preferences unavailable'));
  controller.start();
  await flush();
  expect(controller.getSnapshot().status?.state).toBe('recording');
  expect(controller.getSnapshot().settingsError).toBe('Preferences unavailable');
  controller.command({ action: 'stop' }, 9);
  await flush();
  expect(write).toHaveBeenCalledExactlyOnceWith('graph-a', 'recorder-1', { action: 'stop' }, 9);
  controller.dispose();
});

it('captures the initiating revision for a native folder choice instead of rebasing after return', async () => {
  vi.useFakeTimers();
  const { controller, port } = fixture();
  const pending = deferred<string | null>();
  Object.assign(port, { selectRecordingFolder: vi.fn().mockReturnValue(pending.promise) });
  const save = vi.spyOn(port, 'setRecordingSettings');
  controller.start();
  await flush();
  controller.chooseFolder('Choose folder', 41);
  await flush();
  pending.resolve('/Captured');
  await flush();
  expect(save).toHaveBeenCalledExactlyOnceWith(
    'graph-a',
    'recorder-1',
    {
      folder: '/Captured',
      sampleRate: null,
      encoding: 'pcm16',
    },
    41,
  );
  controller.dispose();
});

it('never saves a late native folder result after the target owner is replaced', async () => {
  vi.useFakeTimers();
  const { controller, port } = fixture();
  const pending = deferred<string | null>();
  const choose = vi.fn().mockReturnValue(pending.promise);
  Object.assign(port, { selectRecordingFolder: choose });
  const save = vi.spyOn(port, 'setRecordingSettings');
  controller.start();
  await flush();
  controller.chooseFolder('Choose folder', 1);
  await flush();
  controller.dispose();
  const replacement = new RecorderController(port, 'graph-b', 'recorder-2', timerScheduler);
  replacement.start();
  await flush();
  pending.resolve('/wrong-target');
  await flush();
  expect(save).not.toHaveBeenCalled();
  expect(replacement.getSnapshot().settings?.folder).toBe('/Desktop');
  replacement.dispose();
});

it('ignores late settings loads and save acknowledgments after disposal', async () => {
  vi.useFakeTimers();
  const { controller, port } = fixture();
  const pending = deferred<RecorderSettings>();
  vi.spyOn(port, 'recordingSettings').mockReturnValueOnce(pending.promise);
  controller.start();
  await flush();
  controller.dispose();
  pending.resolve({ folder: '/old', sampleRate: null, encoding: 'pcm16' });
  await flush();
  expect(controller.getSnapshot().settings).toBeNull();
  controller.start();
  await flush();
  const save = deferred<RecorderSettings>();
  vi.spyOn(port, 'setRecordingSettings').mockReturnValueOnce(save.promise);
  controller.saveSettings({ folder: '/late' }, 1);
  await flush();
  controller.dispose();
  save.resolve({ folder: '/late', sampleRate: null, encoding: 'pcm16' });
  await flush();
  expect(controller.getSnapshot().settings?.folder).toBe('/Desktop');
});

it('preserves canonical settings on folder cancellation and rejects edits during active takes', async () => {
  vi.useFakeTimers();
  const { controller, port, read } = fixture();
  Object.assign(port, { selectRecordingFolder: vi.fn().mockResolvedValue(null) });
  const save = vi.spyOn(port, 'setRecordingSettings');
  controller.start();
  await flush();
  controller.chooseFolder('Choose folder', 1);
  await flush();
  expect(save).not.toHaveBeenCalled();
  expect(controller.getSnapshot().busy).toBe(false);
  read.mockResolvedValue({ ...stopped, state: 'paused' });
  await vi.advanceTimersByTimeAsync(1000);
  controller.saveSettings({ folder: '/ignored' }, 1);
  await flush();
  expect(save).not.toHaveBeenCalled();
  controller.dispose();
});
afterEach(() => vi.useRealTimers());

describe('RecorderController ownership', () => {
  it('keeps a restarted owner isolated from the preceding in-flight response', async () => {
    vi.useFakeTimers();
    const { controller, read } = fixture();
    let resolve: (status: RecorderStatus) => void = () => undefined;
    read.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    controller.start();
    await flush();
    controller.dispose();
    controller.start();
    resolve({ ...stopped, state: 'recording', frames: 100 });
    await flush();
    expect(read).toHaveBeenCalledTimes(2);
    expect(controller.getSnapshot().status).toEqual(stopped);
    controller.dispose();
  });
});

describe('RecorderController failures', () => {
  it('retains the confirmed take on a busy read but surfaces actual faults and deleted nodes', async () => {
    vi.useFakeTimers();
    const { controller, read } = fixture();
    controller.start();
    await flush();
    read.mockRejectedValueOnce(
      new ControlPortError(409, { code: 'graph_exchange_busy', parameters: {} }),
    );
    await vi.advanceTimersByTimeAsync(1000);
    expect(controller.getSnapshot()).toMatchObject({ status: stopped, busy: false, error: null });
    read.mockRejectedValueOnce(new Error('Disk failure'));
    await vi.advanceTimersByTimeAsync(1000);
    expect(controller.getSnapshot().error).toBe('Disk failure');
    read.mockRejectedValueOnce(
      new ControlPortError(404, { code: 'node_not_found', parameters: {} }),
    );
    await vi.advanceTimersByTimeAsync(1000);
    expect(controller.getSnapshot().status).toBeNull();
    expect(controller.getSnapshot().error).not.toBeNull();
    controller.dispose();
  });

  it('does not silently retry a busy or disabled recorder command', async () => {
    vi.useFakeTimers();
    const { controller, write } = fixture();
    controller.start();
    await flush();
    write.mockRejectedValueOnce(
      new ControlPortError(409, { code: 'graph_exchange_busy', parameters: { reason: 'busy' } }),
    );
    controller.command({ action: 'record' }, 1);
    await flush();
    expect(controller.getSnapshot().error).toBe('busy');
    await vi.advanceTimersByTimeAsync(2000);
    expect(write).toHaveBeenCalledTimes(1);
    expect(controller.getSnapshot().error).toBe('busy');
    write.mockRejectedValueOnce(
      new ControlPortError(409, { code: 'node_disabled', parameters: { reason: 'off' } }),
    );
    controller.command({ action: 'record' }, 1);
    await flush();
    expect(controller.getSnapshot().error).toBe('off');
    expect(controller.getSnapshot().status).toEqual(stopped);
    controller.dispose();
  });
});

describe('RecorderController requests', () => {
  it('serializes status and commands, rejecting duplicate button presses', async () => {
    vi.useFakeTimers();
    const { controller, read, write } = fixture();
    let resolve: (status: RecorderStatus) => void = () => undefined;
    read.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    controller.start();
    await flush();
    // Initial settings are not yet known: an early command must not create a take.
    controller.command({ action: 'record' }, 1);
    expect(write).not.toHaveBeenCalled();
    resolve(stopped);
    await flush();
    controller.command({ action: 'record' }, 1);
    controller.command({ action: 'record' }, 1);
    await flush();
    expect(write).toHaveBeenCalledExactlyOnceWith('graph-a', 'recorder-1', { action: 'record' }, 1);
    expect(controller.getSnapshot().status?.state).toBe('recording');
    controller.dispose();
  });

  it('preserves the last status after a rejected command and allows retry', async () => {
    vi.useFakeTimers();
    const { controller, write } = fixture();
    controller.start();
    await flush();
    write.mockRejectedValueOnce(new Error('Disk full'));
    controller.command({ action: 'record' }, 1);
    await flush();
    expect(controller.getSnapshot()).toMatchObject({
      status: stopped,
      busy: false,
      error: 'Disk full',
    });
    await vi.advanceTimersByTimeAsync(1000);
    expect(controller.getSnapshot().error).toBe('Disk full');
    controller.command({ action: 'record' }, 1);
    await flush();
    expect(controller.getSnapshot().error).toBeNull();
    controller.dispose();
  });

  it('ignores late completion after disposal and never continues polling', async () => {
    vi.useFakeTimers();
    const { controller, read } = fixture();
    let resolve: (status: RecorderStatus) => void = () => undefined;
    read.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    controller.start();
    await flush();
    controller.dispose();
    resolve(stopped);
    await flush();
    await vi.advanceTimersByTimeAsync(5000);
    expect(controller.getSnapshot().status).toBeNull();
    expect(read).toHaveBeenCalledTimes(1);
  });

  it('does not overlap slow polling reads', async () => {
    vi.useFakeTimers();
    const { controller, read } = fixture();
    read.mockImplementation(() => new Promise(() => undefined));
    controller.start();
    await flush();
    await vi.advanceTimersByTimeAsync(5000);
    expect(read).toHaveBeenCalledTimes(1);
    controller.dispose();
  });
});

it('derives duration from saved frames only', () => {
  expect(recordingDuration({ ...stopped, frames: 48000 * 3661, sampleRate: 48000 })).toBe(
    '01:01:01.000',
  );
  expect(recordingDuration(null)).toBe('00:00:00.000');
  expect(recordingDuration({ ...stopped, frames: 59952, sampleRate: 48000 })).toBe('00:00:01.249');
  expect(recordingDuration({ ...stopped, frames: 4410, sampleRate: 44100 })).toBe('00:00:00.100');
});

it('polls active recording every 100ms without extrapolating frames, then slows while paused', async () => {
  vi.useFakeTimers();
  const { controller, read, write } = fixture();
  const recording: RecorderStatus = {
    ...stopped,
    state: 'recording',
    frames: 59952,
    sampleRate: 48000,
  };
  read.mockResolvedValue(recording);
  controller.start();
  await flush();
  await vi.advanceTimersByTimeAsync(99);
  expect(read).toHaveBeenCalledTimes(1);
  expect(recordingDuration(controller.getSnapshot().status)).toBe('00:00:01.249');
  await vi.advanceTimersByTimeAsync(1);
  expect(read).toHaveBeenCalledTimes(2);
  const paused: RecorderStatus = { ...recording, state: 'paused' };
  write.mockResolvedValue(paused);
  read.mockResolvedValue(paused);
  controller.command({ action: 'pause' }, 1);
  await flush();
  await vi.advanceTimersByTimeAsync(999);
  expect(read).toHaveBeenCalledTimes(2);
  expect(recordingDuration(controller.getSnapshot().status)).toBe('00:00:01.249');
  await vi.advanceTimersByTimeAsync(1);
  expect(read).toHaveBeenCalledTimes(3);
  controller.dispose();
});
