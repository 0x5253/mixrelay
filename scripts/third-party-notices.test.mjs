import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { javascriptPackages, renderNotices, rustPackages } from './third-party-notices.mjs';

const crate = (directory, name, license, kinds = ['lib']) => ({
  id: name, name, version: '1.0.0', license, license_file: null,
  manifest_path: join(directory, name, 'Cargo.toml'), targets: [{ kind: kinds }],
});

test('Rust notices follow normal dependencies of shipped workspace artifacts only', () => {
  const directory = mkdtempSync(join(tmpdir(), 'notices-rust-'));
  for (const name of ['shipped', 'tooling', 'runtime', 'builder', 'tester', 'transitive', 'tool-only']) mkdirSync(join(directory, name));
  writeFileSync(join(directory, 'runtime', 'LICENSE-MIT'), 'MIT text\r\n');
  const normal = [{ kind: null, target: null }];
  const metadata = {
    workspace_members: ['shipped', 'tooling'],
    packages: [crate(directory, 'shipped', null, ['bin']), crate(directory, 'tooling', null),
      crate(directory, 'runtime', 'MIT'), crate(directory, 'builder', 'MIT'), crate(directory, 'tester', 'MIT'),
      crate(directory, 'transitive', 'Apache-2.0'), crate(directory, 'tool-only', 'MIT')],
    resolve: { nodes: [
      { id: 'shipped', deps: [{ pkg: 'runtime', dep_kinds: normal }, { pkg: 'builder', dep_kinds: [{ kind: 'build' }] },
        { pkg: 'tester', dep_kinds: [{ kind: 'dev' }] }] },
      { id: 'runtime', deps: [{ pkg: 'transitive', dep_kinds: normal }] },
      { id: 'tooling', deps: [{ pkg: 'tool-only', dep_kinds: normal }] },
    ] },
  };
  const packages = rustPackages(metadata);
  assert.deepEqual(packages.map(item => item.name).sort(), ['runtime', 'transitive']);
  assert.deepEqual(packages.find(item => item.name === 'runtime').texts, ['MIT text\n']);
});

test('JavaScript notices resolve production dependencies through nested node_modules', () => {
  const directory = mkdtempSync(join(tmpdir(), 'notices-js-'));
  const write = (path, manifest, license) => {
    mkdirSync(path, { recursive: true });
    writeFileSync(join(path, 'package.json'), JSON.stringify(manifest));
    if (license) writeFileSync(join(path, 'LICENSE'), license);
  };
  write(join(directory, 'app'), { dependencies: { a: '1' }, devDependencies: { dev: '1' } });
  write(join(directory, 'app/node_modules/a'), { name: 'a', version: '1.0.0', license: 'MIT', dependencies: { b: '1' } }, 'A licence');
  write(join(directory, 'app/node_modules/a/node_modules/b'), { name: 'b', version: '2.0.0', license: { type: 'ISC' } });
  write(join(directory, 'app/node_modules/dev'), { name: 'dev', version: '1.0.0', license: 'MIT' });
  const packages = javascriptPackages(join(directory, 'app'));
  assert.deepEqual(packages.map(item => `${item.name}@${item.version} ${item.license}`), ['a@1.0.0 MIT', 'b@2.0.0 ISC']);
});

test('rendered notices list every package and write each distinct licence text once', () => {
  const text = renderNotices({
    rust: [{ name: 'zeta', version: '1.0.0', license: 'MIT', texts: ['Shared\n'] },
      { name: 'alpha', version: '1.0.0', license: 'MIT', texts: ['Shared\n'] },
      { name: 'alpha', version: '1.0.0', license: 'MIT', texts: ['Shared\n'] }],
    javascript: [{ name: 'bare', version: '3.0.0', license: 'MIT', texts: [] }],
  });
  assert.match(text, /Rust crates \(2\)\n- alpha 1\.0\.0 \(MIT\)\n- zeta 1\.0\.0 \(MIT\)/);
  assert.match(text, /JavaScript packages \(1\)\n- bare 3\.0\.0 \(MIT\)/);
  assert.match(text, /ship no separate licence file[^\n]*\n- bare 3\.0\.0/);
  assert.equal(text.match(/^Applies to:/gm).length, 1);
  assert.match(text, /Applies to: alpha 1\.0\.0, zeta 1\.0\.0\n-+\nShared\n$/);
});
