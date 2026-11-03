import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { interactionSites } from './audit-ui.mjs';

test('lists native aliases, imported React Aria aliases and inherited names without inventing names', () => {
  const sites = interactionSites(`
    import { Button as AriaButton } from 'react-aria-components';
    function Card() {
      const Face = selected ? 'button' : 'div';
      return <>
        <AriaButton aria-label={title} />
        <Face aria-label={title} />
        <IconActionButton label={t('retry')} />
        <button {...props} />
        <button aria-hidden="true" tabIndex={-1} />
        <div>Label</div>
      </>;
    }
  `);
  assert.equal(sites.length, 5);
  assert.equal(sites[0].resolved, 'Button');
  assert.equal(sites[1].resolved, 'button|div');
  assert.equal(sites[2].name, "{t('retry')}");
  assert.equal(sites[3].inherited, true);
  assert.equal(sites[3].name, null);
  assert.equal(sites[4].hidden, '"true"');
  assert.ok(sites.every((site) => site.owner === 'Card' && site.line > 0));
});

test('help creates no report and unknown options fail clearly', () => {
  const cli = fileURLToPath(new URL('./audit-ui.mjs', import.meta.url));
  const help = spawnSync(process.execPath, [cli, '--help'], { encoding: 'utf8' });
  assert.equal(help.status, 0);
  assert.match(help.stdout, /Usage:.*audit:ui/);
  assert.match(help.stdout, /no files are created/);
  const invalid = spawnSync(process.execPath, [cli, '--unknown'], { encoding: 'utf8' });
  assert.notEqual(invalid.status, 0);
  assert.match(invalid.stderr, /Unknown option. Use --help/);
});
