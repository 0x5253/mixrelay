import assert from 'node:assert/strict';
import { once } from 'node:events';
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { socketDirectory, engineSocket, driverSocket, socketRequest, socketJson } from './local-socket.mjs';

test('absolute IPC override takes precedence on both platforms', () => {
  for (const platform of ['darwin', 'linux']) {
    const env = { MIXRELAY_IPC_DIR: '/private/lab ipc', HOME: '/ignored', XDG_RUNTIME_DIR: '/ignored' };
    assert.equal(socketDirectory(env, platform), env.MIXRELAY_IPC_DIR);
    assert.equal(engineSocket(env, platform), '/private/lab ipc/engine.sock');
    assert.equal(driverSocket(env, platform), '/private/lab ipc/driver.sock');
    assert.throws(() => socketDirectory({ ...env, MIXRELAY_IPC_DIR: '../ipc' }, platform), /must be an absolute path/);
  }
});

test('macOS sockets use the user Application Support directory', () => {
  const env = { HOME: '/Users/lab', XDG_RUNTIME_DIR: '/ignored' };
  const directory = '/Users/lab/Library/Application Support/MixRelay/ipc';
  assert.equal(socketDirectory(env, 'darwin'), directory);
  assert.equal(engineSocket(env, 'darwin'), join(directory, 'engine.sock'));
  assert.equal(driverSocket(env, 'darwin'), join(directory, 'driver.sock'));
  assert.equal(socketDirectory({}, 'darwin'), join(homedir(), 'Library/Application Support/MixRelay/ipc'));
});

test('Linux sockets require an absolute session runtime directory', () => {
  const env = { XDG_RUNTIME_DIR: '/run/user/1000', HOME: '/ignored' };
  assert.equal(socketDirectory(env, 'linux'), '/run/user/1000/mixrelay');
  assert.equal(engineSocket(env, 'linux'), '/run/user/1000/mixrelay/engine.sock');
  assert.equal(driverSocket(env, 'linux'), '/run/user/1000/mixrelay/driver.sock');
  for (const runtime of [undefined, '', 'relative/runtime']) {
    assert.throws(() => socketDirectory({ XDG_RUNTIME_DIR: runtime }, 'linux'), /XDG_RUNTIME_DIR/);
  }
});

async function serverFixture(t) {
  // Keep sockaddr_un below the macOS path limit, including its long TMPDIR prefix.
  const directory = await mkdtemp(join(tmpdir(), 'mr-'));
  const server = createServer(async (request, response) => {
    let body = '';
    for await (const chunk of request) body += chunk;
    if (request.url === '/empty') {
      response.writeHead(204).end();
    } else if (request.url === '/error') {
      response.writeHead(409).end('revision conflict');
    } else if (request.url === '/invalid-json') {
      response.end('not json');
    } else if (request.url !== '/timeout') {
      response.writeHead(202, { 'content-type': 'application/json', 'x-socket-test': 'yes' });
      response.end(JSON.stringify({ method: request.method, path: request.url, headers: request.headers, body }));
    }
  });
  t.after(async () => {
    try {
      if (server.listening) {
        server.closeAllConnections();
        await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
      }
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
  const socket = engineSocket({ MIXRELAY_IPC_DIR: directory });
  server.listen(socket);
  await once(server, 'listening');
  return socket;
}

test('socketRequest sends HTTP method, path, Host and body over a real Unix socket', async t => {
  const socket = await serverFixture(t);
  const response = await socketRequest(socket, '/api/v1/graph?revision=7', {
    method: 'PUT', headers: { host: 'mixrelay-engine', 'content-type': 'text/plain' }, body: '한글 payload',
  });
  assert.equal(response.status, 202);
  assert.equal(response.headers['x-socket-test'], 'yes');
  const request = JSON.parse(response.body);
  assert.equal(request.method, 'PUT');
  assert.equal(request.path, '/api/v1/graph?revision=7');
  assert.equal(request.headers.host, 'mixrelay-engine');
  assert.equal(request.headers.origin, undefined);
  assert.equal(request.headers['content-type'], 'text/plain');
  assert.equal(request.body, '한글 payload');
});

test('socketJson encodes JSON, sends the Driver Host without Origin and decodes JSON', async t => {
  const socket = await serverFixture(t);
  const request = await socketJson(socket, '/api/v1/command', {
    method: 'POST', headers: { host: 'mixrelay-driver' }, json: { enabled: false, label: '소켓' },
  });
  assert.equal(request.method, 'POST');
  assert.equal(request.headers.host, 'mixrelay-driver');
  assert.equal(request.headers.origin, undefined);
  assert.equal(request.headers['content-type'], 'application/json');
  assert.deepEqual(JSON.parse(request.body), { enabled: false, label: '소켓' });
  const get = await socketJson(socket, '/api/v1/status', { headers: { host: 'mixrelay-engine' } });
  assert.equal(get.method, 'GET');
  assert.equal(get.body, '');
});

test('socketJson handles empty success, HTTP errors and malformed JSON', async t => {
  const socket = await serverFixture(t);
  assert.equal(await socketJson(socket, '/empty'), undefined);
  assert.equal((await socketRequest(socket, '/error')).status, 409);
  await assert.rejects(socketJson(socket, '/error'), /\/error: HTTP 409 revision conflict/);
  await assert.rejects(socketJson(socket, '/invalid-json'), SyntaxError);
});

test('socket requests reject connection failures and stalled responses', async t => {
  const socket = await serverFixture(t);
  await assert.rejects(socketRequest(`${socket}.missing`, '/health'), { code: 'ENOENT' });
  await assert.rejects(socketRequest(socket, '/timeout', { timeoutMs: 50 }), /\/timeout: timed out/);
});
