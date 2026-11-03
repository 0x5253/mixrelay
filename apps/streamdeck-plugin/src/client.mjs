import { ControlFrames, encodeCommand } from './control-frames.mjs';
import { createConnection } from 'node:net';
import { homedir } from 'node:os';
import { join } from 'node:path';

export const socketPath = join(
  homedir(),
  'Library/Application Support/io.mixrelay.desktop/control/control.sock',
);

/** One bounded exchange. A failed write is never retried. */
export function exchange(command, path = socketPath, timeout = 35000) {
  return new Promise((resolve, reject) => {
    const socket = createConnection(path);
    const frames = new ControlFrames();
    let settled = false;
    let timer;
    const resetTimer = () => {
      clearTimeout(timer);
      timer = setTimeout(() => finish(new Error('MixRelay did not respond')), timeout);
    };
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      if (error) reject(error);
      else resolve(value);
    };
    resetTimer();
    socket.on('error', (error) => finish(error));
    socket.on('end', () => finish(new Error('MixRelay disconnected')));
    socket.on('connect', () => {
      socket.write(encodeCommand(command));
      resetTimer();
    });
    socket.on('data', (chunk) => {
      if (settled) return;
      try {
        for (const response of frames.accept(chunk)) {
          resetTimer();
          if (response.pending) continue;
          finish(null, response.body);
          break;
        }
      } catch (error) {
        finish(error);
      }
    });
  });
}
