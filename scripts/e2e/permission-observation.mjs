import { permissionHandshake } from '../core/permission-handshake.mjs';
import { randomUUID } from 'node:crypto';
import { existsSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

export function permissionObservation(directory, alive, { now = Date.now, wait = delay } = {}) {
  return permissionHandshake({
    directory, alive, now, wait, nextId: randomUUID,
    channel: {
      publish: (command) => {
        const pending = join(directory, 'permission-observation.pending.json');
        writeFileSync(pending, JSON.stringify(command), { flag: 'wx' });
        renameSync(pending, join(directory, 'permission-observation.command.json'));
      },
      read: () => {
        const acknowledgement = join(directory, 'permission-observation.ack.json');
        return existsSync(acknowledgement) ? JSON.parse(readFileSync(acknowledgement, 'utf8')) : undefined;
      },
    },
    writeReceipt: (receiptDirectory, receipt) => writeFileSync(join(receiptDirectory, 'native-permission-observation.json'),
      JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx' }),
  });
}
