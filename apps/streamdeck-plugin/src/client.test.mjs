import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { exchange } from './client.mjs';

async function fixture(run, reply) {
  const folder = await mkdtemp(join(tmpdir(), 'mr-ipc-'));
  const path = join(folder, 'control.sock');
  const server = createServer((socket) => {
    socket.once('error', () => {});
    socket.once('data', (data) => reply(socket, JSON.parse(data)));
  });
  await new Promise((resolve) => server.listen(path, resolve));
  try {
    await run(path);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(folder, { recursive: true });
  }
}
test('bounded newline response supports fragmented UTF-8', async () => {
  await fixture(
    async (path) => assert.deepEqual(await exchange({ type: 'snapshot' }, path), { label: '녹음' }),
    (socket, request) => {
      assert.deepEqual(request, { id: 1, command: { type: 'snapshot' } });
      const bytes = Buffer.from('{"id":1,"status":200,"body":{"label":"녹음"}}\n');
      const split = bytes.indexOf(Buffer.from('녹음')) + 1;
      socket.write(bytes.subarray(0, split));
      socket.end(bytes.subarray(split));
    },
  );
});

test('accepted mutation stays alive while complete pending frames reset inactivity', async () => {
  await fixture(
    async (path) =>
      assert.deepEqual(
        await exchange({ type: 'node' }, path, 100),
        { revision: 10, state: 'pending' },
      ),
    (socket) => {
      let heartbeats = 5;
      const timer = setInterval(() => {
        socket.write('{"id":1,"status":102,"body":{}}\n');
        heartbeats--;
        if (heartbeats === 0) {
          clearInterval(timer);
          socket.end('{"id":1,"status":202,"body":{"revision":10,"state":"pending"}}\n');
        }
      }, 30);
    },
  );
});

test('a stalled pending frame times out without replaying the command', async () => {
  let received = 0;
  await fixture(
    async (path) => {
      await assert.rejects(exchange({ type: 'node' }, path, 25), /MixRelay did not respond/);
      assert.equal(received, 1);
    },
    (socket) => {
      received++;
      socket.write('{"id":1,"status":102,"body":{}}\n');
    },
  );
});

test('fragmented and coalesced frames are parsed independently', async () => {
  await fixture(
    async (path) =>
      assert.deepEqual(await exchange({ type: 'node' }, path, 100), { revision: 11 }),
    (socket) =>
      socket.end(
        '{"id":1,"status":102,"body":{}}\n{"id":1,"status":200,"body":{"revision":11}}\n',
      ),
  );
});

test('a fragmented frame cannot exceed the per-frame response limit', async () => {
  await fixture(
    // The size limit, not the response timeout, must end this exchange; 4 MiB in 1 KiB
    // writes can take longer than a short timeout on a loaded machine.
    async (path) =>
      await assert.rejects(exchange({ type: 'snapshot' }, path, 30000), /Response too large/),
    (socket) => {
      const bytes = Buffer.from(
        `{"id":1,"status":200,"body":{"data":"${'x'.repeat(4 * 1024 * 1024)}"}}\n`,
      );
      for (let offset = 0; offset < bytes.length; offset += 1024)
        socket.write(bytes.subarray(offset, Math.min(offset + 1024, bytes.length)));
    },
  );
});

test('a trailing partial frame remains bounded until its delimiter arrives', async () => {
  await fixture(
    async (path) =>
      assert.deepEqual(await exchange({ type: 'node' }, path, 100), { revision: 12 }),
    (socket) => {
      socket.write(
        '{"id":1,"status":102,"body":{}}\n{"id":1,"status":200,"body":{"revision":',
      );
      setTimeout(() => socket.end('12}}\n'), 10);
    },
  );
});

test('rejected and uncertain writes are sent once, never replayed', async () => {
  let received = 0;
  await fixture(
    async (path) => {
      await assert.rejects(exchange({ type: 'node' }, path), /graph_revision_stale/);
      assert.equal(received, 1);
    },
    (socket) => {
      received++;
      socket.end('{"id":1,"status":409,"body":{"code":"graph_revision_stale"}}\n');
    },
  );
  await fixture(
    async (path) => {
      await assert.rejects(exchange({ type: 'node' }, path));
      assert.equal(received, 2);
    },
    (socket) => {
      received++;
      socket.end();
    },
  );
});

test('accepted node publication is a successful acknowledgement, not an active-state promise', async () => {
  await fixture(
    async (path) =>
      assert.deepEqual(await exchange({ type: 'node' }, path), { revision: 9, state: 'pending' }),
    (socket) => socket.end('{"id":1,"status":202,"body":{"revision":9,"state":"pending"}}\n'),
  );
});
