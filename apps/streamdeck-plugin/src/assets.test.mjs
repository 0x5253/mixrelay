import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { generateIcons } from '../icons.mjs';

test('brand sources generate the required plugin PNG sizes and a transparent white category SVG', async t => {
  const output = await mkdtemp(join(tmpdir(), 'mixrelay-streamdeck-icons-'));
  t.after(() => rm(output, { recursive: true, force: true }));
  await mkdir(join(output, 'images'));
  await generateIcons(output, join(output, 'marketplace'));
  for (const [suffix, size] of [['', 256], ['@2x', 512]]) {
    const bytes = await readFile(join(output, `images/app${suffix}.png`));
    assert.equal(bytes.subarray(1, 4).toString(), 'PNG');
    assert.equal(bytes.readUInt32BE(16), size);
    assert.equal(bytes.readUInt32BE(20), size);
  }
  const listing = await readFile(join(output, 'marketplace/app-icon.png'));
  assert.equal(listing.readUInt32BE(16), 288);
  assert.equal(listing.readUInt32BE(20), 288);
  const category = await readFile(join(output, 'images/icon.svg'), 'utf8');
  assert.match(category, /width="28" height="28"/);
  assert.match(category, /fill="#FFFFFF"/);
  assert.doesNotMatch(category, /<rect|<image|stroke=/);
  const source = await readFile('../../branding/identity/symbols/mixrelay-white.svg', 'utf8');
  assert.equal(category.match(/ d="([^"]+)"/)[1], source.match(/ d="([^"]+)"/)[1]);
  for (const action of ['power', 'mute', 'volume', 'recording']) {
    const svg = await readFile(`images/${action}.svg`, 'utf8');
    assert.match(svg, /width="20" height="20"/);
    assert.match(svg, /stroke="#FFFFFF"/);
    const key = await readFile(`dist/com.rawsector.mixrelay.sdPlugin/images/key-${action}.svg`, 'utf8');
    assert.match(key, /width="72" height="72"/);
  }
  const manifest = JSON.parse(await readFile('dist/com.rawsector.mixrelay.sdPlugin/manifest.json'));
  assert.ok(manifest.Actions.every(action => action.Tooltip && action.UserTitleEnabled === false));
  assert.equal(manifest.Actions.find(action => action.UUID.endsWith('.volume')).Encoder.layout, '$B1');
});
