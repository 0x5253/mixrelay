import { createConnection } from 'node:net';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { runtimeControl } from './core/runtime-control.mjs';

export const invoke = runtimeControl({
  connect: createConnection,
  schedule: setTimeout,
  cancel: clearTimeout,
  path: join(homedir(), 'Library/Application Support/io.mixrelay.desktop/control/control.sock'),
});
