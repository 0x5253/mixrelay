import { spawnSync } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export function prepareCompilerCache(run = spawnSync, environment = process.env) {
  const common = { SCCACHE_IGNORE_SERVER_IO_ERROR: '1' };
  const probe = env => run('sccache', ['rustc', '-vV'], { env, encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'], timeout: 15000, maxBuffer: 1024 * 1024 });
  const successful = result => !result.error && !result.signal && result.status === 0;
  if (successful(probe({ ...environment, ...common }))) return { mode: 'github-actions', environment: common };
  // Cache diagnostics can contain signed storage URLs. Keep them out of public logs.
  run('sccache', ['--stop-server'], { env: environment, stdio: 'ignore', timeout: 15000 });
  if (!environment.RUNNER_TEMP) throw new Error('RUNNER_TEMP is required for local compiler cache fallback');
  const local = { ...common, SCCACHE_GHA_ENABLED: 'off', SCCACHE_DIR: join(environment.RUNNER_TEMP, 'sccache-local'),
    SCCACHE_CACHE_SIZE: '1G' };
  if (!successful(probe({ ...environment, ...local }))) throw new Error('Compiler/cache preflight failed with both remote and local storage');
  return { mode: 'local', environment: local };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (!process.env.GITHUB_ENV) throw new Error('This preflight is for GitHub-hosted CI');
    const result = prepareCompilerCache();
    appendFileSync(process.env.GITHUB_ENV, Object.entries({ ...result.environment, MIXRELAY_COMPILER_CACHE: result.mode })
      .map(([key, value]) => `${key}=${value}\n`).join(''));
    console.log(`Compiler cache: ${result.mode}${result.mode === 'local' ? ' (remote storage unavailable; all build/test checks remain required)' : ''}`);
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
