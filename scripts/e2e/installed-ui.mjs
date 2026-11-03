import assert from 'node:assert/strict';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { spawnLogged } from './package.mjs';
import { permissionObservation } from './permission-observation.mjs';

export async function startInstalledUi(os, output) {
  const directory = resolve(output);
  const log = join(directory, 'installed-ui.log');
  const child = os === 'macos'
    ? spawnLogged('bash', ['scripts/e2e/macos-ui.sh', directory], log, { detached: true })
    : spawnLogged('python3', ['scripts/e2e/linux-ui.py', directory], log, { detached: true });
  let spawnError;
  child.on('error', error => { spawnError = error; });
  const completed = new Promise(resolveExit => child.once('close', (code, signal) => resolveExit({ code, signal })));
  const alive = () => !spawnError && child.exitCode === null && child.signalCode === null;
  return {
    measurementBoundary: os === 'macos' ? permissionObservation(directory, alive) : measure => measure(),
    async ready() {
      const deadline = Date.now() + (process.env.MIXRELAY_E2E_VM_ACCELERATOR === 'tcg' ? 900000 : 300000);
      while (!existsSync(join(directory, 'installed-ui.ready')) && alive() && Date.now() < deadline) await delay(100);
      assert.ifError(spawnError);
      assert.ok(existsSync(join(directory, 'installed-ui.ready')), `Installed UI failed: ${readFileSync(log, 'utf8').slice(-5000)}`);
      const failure = join(directory, 'installed-ui.error');
      assert.ok(!existsSync(failure), existsSync(failure) ? readFileSync(failure, 'utf8') : '');
    },
    async finish() {
      writeFileSync(join(directory, 'installed-ui.finish'), 'finish\n', { flag: 'wx' });
      const result = await Promise.race([completed, delay(30000).then(() => null)]);
      if (!result) {
        const terminate = signal => {
          try { process.kill(-child.pid, signal); } catch (error) { if (error.code !== 'ESRCH') throw error; }
        };
        terminate('SIGTERM');
        await Promise.race([completed, delay(5000)]);
        terminate('SIGKILL');
        await Promise.race([completed, delay(5000)]);
        assert.fail('Installed UI runner did not stop');
      }
      return { passed: result.code === 0 && result.signal === null,
        error: `Installed UI runner failed: ${readFileSync(log, 'utf8').slice(-5000)}` };
    },
  };
}
