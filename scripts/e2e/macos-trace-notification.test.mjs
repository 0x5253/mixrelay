import assert from 'node:assert/strict';
import test from 'node:test';
import { EventEmitter } from 'node:events';
import { existsSync } from 'node:fs';
import { dirname } from 'node:path';
import { startTraceNotification } from './macos-trace-notification.mjs';

function fixture() {
  const child = new EventEmitter();
  child.stdout = new EventEmitter(); child.stderr = new EventEmitter();
  child.exitCode = null; child.signalCode = null;
  const stopped = [];
  child.kill = signal => {
    stopped.push(signal); child.signalCode = signal;
    child.emit('exit', null, signal); child.emit('close', null, signal);
  };
  let directory;
  const notification = startTraceNotification({
    execute(program, args) {
      assert.equal(program, 'xcrun'); assert.equal(args[0], 'clang');
      directory = dirname(args.at(-1)); assert.equal(existsSync(directory), true);
    },
    start(program, args) {
      assert.match(program, /observer$/);
      assert.match(args[0], /^io\.mixrelay\.diagnostics\.trace\./);
      return child;
    },
  });
  return { child, stopped, notification, directory };
}

test('observer registration and start are separate; completed observers leave no helper files', async () => {
  const { child, notification, directory, stopped } = fixture();
  try {
    assert.equal(notification.registered(), false);
    child.stdout.emit('data', Buffer.from('registered\n'));
    assert.equal(notification.registered(), true);
    assert.equal(notification.started(), false);
    child.stdout.emit('data', Buffer.from('started\n'));
    child.exitCode = 0; child.emit('close', 0, null);
    assert.equal(notification.started(), true);
  } finally { await notification.stop(); }
  assert.equal(existsSync(directory), false);
  assert.deepEqual(stopped, []);
});

test('observer failure and bounded output never authorize measurement and live helpers stop', async () => {
  for (const fail of ['exit', 'output', 'spawn']) {
    const { child, notification, directory, stopped } = fixture();
    try {
      if (fail === 'exit') { child.exitCode = 1; child.emit('close', 1, null); }
      if (fail === 'output') child.stdout.emit('data', Buffer.alloc(4097));
      if (fail === 'spawn') child.emit('error', new Error('observer spawn failed'));
      assert.throws(notification.registered);
      assert.throws(notification.started);
    } finally { await notification.stop(); }
    assert.equal(existsSync(directory), false);
    assert.deepEqual(stopped, fail === 'exit' ? [] : ['SIGTERM']);
  }
});

test('compiler and synchronous spawn failures reclaim the unstarted helper directory', () => {
  for (const fail of ['compile', 'spawn']) {
    let directory;
    assert.throws(() => startTraceNotification({
      execute(_, args) {
        directory = dirname(args.at(-1));
        if (fail === 'compile') throw new Error('fixture compile failure');
      },
      start() { throw new Error('fixture spawn failure'); },
    }), /fixture .* failure/);
    assert.equal(existsSync(directory), false);
  }
});
