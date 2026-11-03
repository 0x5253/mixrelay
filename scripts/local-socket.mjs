import { httpExchange } from './core/http-exchange.mjs';
// Locates MixRelay's user-owned control sockets and sends HTTP over them, matching
// crates/local-socket: MIXRELAY_IPC_DIR when set (absolute), otherwise
// ~/Library/Application Support/MixRelay/ipc on macOS and $XDG_RUNTIME_DIR/mixrelay on Linux.
import { request as httpRequest } from 'node:http';
import { homedir } from 'node:os';
import { isAbsolute, join } from 'node:path';

export const ENGINE_SOCKET = 'engine.sock';
export const DRIVER_SOCKET = 'driver.sock';

export function socketDirectory(environment = process.env, platform = process.platform) {
  const explicit = environment.MIXRELAY_IPC_DIR;
  if (explicit) {
    if (!isAbsolute(explicit)) throw new Error('MIXRELAY_IPC_DIR must be an absolute path');
    return explicit;
  }
  if (platform === 'darwin') {
    return join(environment.HOME || homedir(), 'Library/Application Support/MixRelay/ipc');
  }
  const runtime = environment.XDG_RUNTIME_DIR;
  if (!runtime || !isAbsolute(runtime)) throw new Error('XDG_RUNTIME_DIR must be set');
  return join(runtime, 'mixrelay');
}

export const engineSocket = (environment, platform) =>
  join(socketDirectory(environment, platform), ENGINE_SOCKET);
export const driverSocket = (environment, platform) =>
  join(socketDirectory(environment, platform), DRIVER_SOCKET);

/** Sends one HTTP request over a Unix socket and resolves with its status, headers and
 * text body. `timeoutMs` bounds the whole exchange. */
export const socketRequest = httpExchange({
  request: httpRequest,
  schedule: setTimeout,
  cancel: clearTimeout,
});

/** Like `socketRequest`, but parses JSON and rejects on a non-2xx status. */
export async function socketJson(socketPath, path, options = {}) {
  const body = options.json === undefined ? options.body : JSON.stringify(options.json);
  const headers =
    options.json === undefined
      ? options.headers
      : { 'content-type': 'application/json', ...options.headers };
  const response = await socketRequest(socketPath, path, { ...options, headers, body });
  if (response.status < 200 || response.status > 299) {
    throw new Error(`${path}: HTTP ${response.status} ${response.body}`);
  }
  return response.body ? JSON.parse(response.body) : undefined;
}
