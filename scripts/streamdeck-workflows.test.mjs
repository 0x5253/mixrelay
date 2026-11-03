import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const workflow = name => readFileSync(new URL(`../.github/workflows/${name}.yml`, import.meta.url), 'utf8');

test('both candidate workflows fetch the companion and preserve archive evidence for their existing report gate', () => {
  for (const [name, prefix] of [['candidate-e2e', 'installed'], ['qualify-release', 'qualification']]) {
    const source = workflow(name);
    const download = source.split('\n').find(line => line.includes('gh release download')
      && line.includes(name === 'candidate-e2e' ? '--dir inputs/macos' : '--dir candidate'));
    assert.match(download, /--pattern '\*\.streamDeckPlugin'/);
    assert.match(source, /pnpm install --frozen-lockfile/);
    assert.match(source, /streamdeck-artifact\.mjs qualify --directory candidate --output streamdeck-evidence/);
    assert.ok(source.includes(`name: ${prefix}-target-streamdeck-plugin`));
    assert.match(source, /path: streamdeck-evidence\//);
    assert.ok(source.includes(`pattern: ${prefix}-target-*`));
    assert.match(source, /scripts\/qualification\.mjs gate/);
  }
});

test('source CI validates the distributable and the official release verifies and publishes its bytes', () => {
  const source = workflow('native-lab');
  assert.match(source, /streamdeck-artifact\.mjs build --output streamdeck-package/);
  assert.match(source, /pnpm --filter @mixrelay\/streamdeck-plugin test/);
  assert.match(source, /path: streamdeck-package\/\*\.streamDeckPlugin/);
  const publication = workflow('release');
  assert.match(publication, /candidate\/\*\.streamDeckPlugin/);
  assert.match(publication, /sha256sum --check SHA256SUMS-/);
  assert.match(publication, /gh release edit .*--draft=false/);
});
