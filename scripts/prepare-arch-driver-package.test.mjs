import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { renderArchRecipe } from './prepare-arch-driver-package.mjs';

test('Arch recipe pins the checked payload and keeps the release binaries intact', () => {
  for (const [architecture, arch] of [['amd64', 'x86_64'], ['arm64', 'aarch64']]) {
    const recipe = renderArchRecipe({ version: '0.1.0', architecture, checksum: 'a'.repeat(64) });
    assert.ok(recipe.includes(`arch=('${arch}')`));
    assert.ok(recipe.includes(`sha256sums=('${'a'.repeat(64)}')`));
    assert.match(recipe, /options=\('!strip' '!debug'\)/);
    assert.match(recipe, /glibc>=2\.39/);
    assert.doesNotMatch(recipe, /@VERSION@|@ARCH@|@SHA256@/);
  }
});

test('makepkg receives the real homepage and named packager with a support address', () => {
  const recipe = renderArchRecipe({ version: '0.1.0', architecture: 'amd64', checksum: 'a'.repeat(64) });
  const result = spawnSync('bash', ['-c', `${recipe}\nprintf '%s\\n' "$url" "$PACKAGER"`], {
    encoding: 'utf8', env: { ...process.env, PACKAGER: 'Unknown Packager' },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.stdout.trim().split('\n'), [
    'https://rawsector.com/products/mixrelay', 'rawsector <support@rawsector.com>',
  ]);
  assert.doesNotMatch(recipe, /@[A-Z]+@/);
});

test('Arch recipe rejects malformed substitutions before shell execution', () => {
  const valid = { version: '0.1.0', architecture: 'amd64', checksum: 'a'.repeat(64) };
  for (const invalid of [
    { version: "0.1.0'; touch injected; '" },
    { version: '../0.1.0' }, { architecture: 'i686' }, { checksum: 'SKIP' },
  ]) assert.throws(() => renderArchRecipe({ ...valid, ...invalid }));
});

test('Arch aborts before replacing/removing an active Driver and scopes the hook to its package', () => {
  const hook = readFileSync(new URL('../packaging/linux/arch/mixrelay-driver.hook', import.meta.url), 'utf8');
  assert.match(hook, /^Operation = Upgrade$/m);
  assert.match(hook, /^Operation = Remove$/m);
  assert.deepEqual(hook.match(/^Target = .*$/gm), ['Target = mix-relay']);
  assert.match(hook, /^When = PreTransaction$/m);
  assert.match(hook, /^AbortOnFail$/m);
  assert.doesNotMatch(hook, /^Operation = Install$/m);
});
