import assert from 'node:assert/strict';
import test from 'node:test';
import { compareSuiteSource } from './suite-source.mjs';

const sourceRevision = '1'.repeat(40), suiteRevision = '2'.repeat(40);
const entry = (path, sha = 'a'.repeat(40), mode = '100644', type = 'blob') => ({ path, mode, sha, type });
const compare = (source, suite, truncated = false) => compareSuiteSource({ sourceRevision, suiteRevision,
  sourceTree: { truncated, sha: '3'.repeat(40), tree: source },
  suiteTree: suite.map(({ path, mode, sha, type }) => `${mode} ${type} ${sha}\t${path}\0`).join(''),
});

test('frozen installers can be exercised by changed suite code and documentation', () => {
  const runtime = entry('apps/engine-service/src/main.rs');
  const result = compare([runtime, entry('scripts/e2e/installed.mjs')],
    [runtime, entry('scripts/e2e/installed.mjs', 'b'.repeat(40)), entry('docs/validation/current.md'),
      entry('.github/workflows/installed-evidence.yml')]);
  assert.equal(result.productSourceMatches, true);
  assert.deepEqual(result.changedSuiteFiles, ['.github/workflows/installed-evidence.yml',
    'docs/validation/current.md', 'scripts/e2e/installed.mjs']);
});

test('runtime, package recipes and build tools require installers from the changed product source', () => {
  for (const path of ['apps/control-ui/src/app/App.tsx', 'native/macos/hal-driver/rust/src/lib.rs',
    'packaging/linux/mixrelay-driver.service', 'scripts/build-linux-release.mjs',
    '.github/workflows/build-linux-release.yml', 'Cargo.lock']) {
    const result = compare([entry(path)], [entry(path, 'b'.repeat(40))]);
    assert.equal(result.productSourceMatches, false);
    assert.deepEqual(result.productChanges, [path]);
  }
});

test('deleted or moved product files, executable mode changes and submodule changes remain product differences', () => {
  const path = 'apps/engine-service/src/main.rs';
  assert.equal(compare([entry(path)], [entry('scripts/e2e/moved.rs')]).productSourceMatches, false);
  assert.equal(compare([entry(path)], [entry(path, 'a'.repeat(40), '100755')]).productSourceMatches, false);
  assert.equal(compare([entry('vendor/plugin', 'a'.repeat(40), '160000', 'commit')],
    [entry('vendor/plugin', 'b'.repeat(40), '160000', 'commit')]).productSourceMatches, false);
});

test('a truncated source tree cannot prove source equivalence', () => {
  assert.throws(() => compare([], [], true), /source tree is incomplete/);
});
