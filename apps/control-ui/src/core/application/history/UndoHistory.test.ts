import { describe, expect, it, vi } from 'vitest';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import { GESTURE_IDLE_MS, HISTORY_LIMIT, MERGE_WINDOW_MS, UndoHistory } from './UndoHistory';

function history<Value = number, Kind = string>() {
  const time = fakeScheduler();
  return { time, history: new UndoHistory<Value, Kind>(time, time) };
}

describe('merge boundaries', () => {
  it('merges one group and kind within the window, up to and including its last millisecond', () => {
    const { time, history: steps } = history();
    steps.record(0, 1, 'gain', 'fader');
    time.advance(MERGE_WINDOW_MS);
    steps.record(1, 2, 'gain', 'fader');
    expect(steps.undo()).toEqual({ kind: 'gain', value: 0 });
    expect(steps.undo()).toBeNull();
  });

  it('starts a new step after the window, for another kind, without a group, or after undo', () => {
    const { time, history: steps } = history();
    steps.record(0, 1, 'gain', 'fader');
    time.advance(MERGE_WINDOW_MS + 1);
    steps.record(1, 2, 'gain', 'fader');
    steps.record(2, 3, 'layout', 'fader');
    steps.record(3, 4, 'layout');
    steps.record(4, 5, 'layout');
    expect([steps.undo(), steps.undo(), steps.undo(), steps.undo()]).toEqual([
      { kind: 'layout', value: 4 },
      { kind: 'layout', value: 3 },
      { kind: 'layout', value: 2 },
      { kind: 'gain', value: 1 },
    ]);
    expect(steps.redo()).toEqual({ kind: 'gain', value: 2 });
    // After navigating, the same gesture records a separate step.
    steps.record(2, 9, 'gain', 'fader');
    expect(steps.undo()).toEqual({ kind: 'gain', value: 2 });
    expect(steps.undo()).toEqual({ kind: 'gain', value: 1 });
  });
});

describe('capacity', () => {
  it('keeps the latest edits up to the limit for immediate and live edits alike', () => {
    const { time, history: steps } = history();
    for (let index = 0; index <= HISTORY_LIMIT; index++) steps.record(index, index + 1, 'gain');
    for (let index = HISTORY_LIMIT; index >= 1; index--)
      expect(steps.undo()).toEqual({ kind: 'gain', value: index });
    expect(steps.undo()).toBeNull();
    steps.clear();
    for (let index = 0; index <= HISTORY_LIMIT; index++) {
      steps.recordDebounced(index, index + 1, 'gain', `gesture-${index}`, 10);
      time.advance(10);
    }
    for (let index = HISTORY_LIMIT; index >= 1; index--)
      expect(steps.undo()).toEqual({ kind: 'gain', value: index });
    expect(steps.undo()).toBeNull();
  });
});

describe('live gestures', () => {
  it('commits a gesture as one step once its control has been idle for the full delay', () => {
    const { time, history: steps } = history();
    for (let value = 1; value <= 500; value += 1) {
      steps.recordDebounced(value - 1, value, 'level', 'input:gain');
      time.advance(1);
    }
    time.advance(GESTURE_IDLE_MS - 2);
    expect(time.pending()).toBe(1);
    time.advance(1);
    expect(time.pending()).toBe(0);
    expect(steps.undo()).toEqual({ kind: 'level', value: 0 });
    expect(steps.undo()).toBeNull();
    expect(steps.redo()).toEqual({ kind: 'level', value: 500 });
  });

  it('commits the previous gesture when another control or kind starts', () => {
    const { time, history: steps } = history();
    steps.recordDebounced(0, 1, 'level', 'compressor:threshold');
    time.advance(20);
    steps.recordDebounced(1, 2, 'level', 'compressor:ratio');
    time.advance(20);
    steps.recordDebounced(2, 3, 'level', 'compressor:threshold');
    steps.recordDebounced(3, 4, 'threshold', 'compressor:threshold');
    expect(
      [steps.undo(), steps.undo(), steps.undo(), steps.undo()].map((step) => step?.value),
    ).toEqual([3, 2, 1, 0]);
    expect(steps.undo()).toBeNull();
  });

  it('flushes an unfinished gesture for undo and drops its scheduled commit', () => {
    const { time, history: steps } = history();
    steps.recordDebounced(0, 42, 'level', 'input:gain');
    expect(steps.undo()).toEqual({ kind: 'level', value: 0 });
    expect(time.pending()).toBe(0);
    time.advance(1000);
    expect(steps.undo()).toBeNull();
    expect(steps.redo()?.value).toBe(42);
  });

  it('starts a new gesture after an idle boundary and invalidates stale redo', () => {
    const { time, history: steps } = history();
    steps.recordDebounced(0, 1, 'level', 'gain');
    time.advance(GESTURE_IDLE_MS);
    steps.recordDebounced(1, 2, 'level', 'gain');
    expect(steps.undo()?.value).toBe(1);
    steps.recordDebounced(1, 3, 'level', 'gain');
    expect(steps.redo()).toBeNull();
    expect(steps.undo()?.value).toBe(1);
    expect(steps.undo()?.value).toBe(0);
  });

  it('places a structural edit after the pending gesture', () => {
    const { history: steps } = history();
    steps.recordDebounced(0, 1, 'level', 'gain');
    steps.record(1, 2, 'remove-node');
    expect(steps.undo()).toEqual({ kind: 'remove-node', value: 1 });
    expect(steps.undo()).toEqual({ kind: 'level', value: 0 });
  });

  it('clear and dispose drop a pending gesture without committing it', () => {
    const { time, history: steps } = history();
    steps.record(0, 1, 'graph');
    steps.recordDebounced(1, 2, 'level', 'gain');
    steps.clear();
    expect(time.pending()).toBe(0);
    expect(steps.undo()).toBeNull();
    expect(steps.redo()).toBeNull();
    steps.recordDebounced(2, 3, 'level', 'gain');
    steps.dispose();
    time.advance(GESTURE_IDLE_MS);
    expect(time.pending()).toBe(0);
    steps.flush();
    expect(steps.undo()).toEqual({ kind: 'level', value: 2 });
  });
});

describe('rebase', () => {
  it('rebases undo, redo and pending entries without losing their boundaries', () => {
    const { history: steps } = history();
    steps.record(0, 1, 'layout');
    steps.record(1, 2, 'layout');
    expect(steps.undo()?.value).toBe(1);
    steps.rebase((value) => value + 10);
    expect(steps.redo()?.value).toBe(12);
    expect(steps.undo()?.value).toBe(11);
    expect(steps.undo()?.value).toBe(10);
    steps.recordDebounced(10, 11, 'gain', 'node');
    steps.rebase((value) => value + 20);
    expect(steps.undo()?.value).toBe(30);
    expect(steps.redo()?.value).toBe(31);
  });

  it('transforms each shared snapshot once so rebased entries keep sharing it', () => {
    const { history: steps } = history<{ value: number }, 'layout'>();
    const [a, b, c] = [{ value: 0 }, { value: 1 }, { value: 2 }];
    steps.record(a, b, 'layout');
    steps.record(b, c, 'layout');
    const transform = vi.fn((snapshot: { value: number }) => ({ value: snapshot.value + 10 }));
    steps.rebase(transform);
    expect(transform).toHaveBeenCalledTimes(3);
    const second = steps.undo();
    const first = steps.undo();
    expect(second?.value).toEqual({ value: 11 });
    expect(steps.redo()?.value).toBe(second?.value);
    expect(first?.value).toEqual({ value: 10 });
  });

  it('keeps a snapshot that rebases to undefined', () => {
    const { history: steps } = history<number | undefined>();
    steps.record(0, 1, 'value');
    steps.rebase(() => undefined);
    expect(steps.undo()).toEqual({ kind: 'value', value: undefined });
  });
});
