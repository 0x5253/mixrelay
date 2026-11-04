import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  inspectCss,
  inspectIntegrationTests,
  inspectScreenRenderTests,
  inspectScript,
  inspectVariableReferences,
  screenRenderTests,
} from './ui-boundary-rules.mjs';

const script = (source) => inspectScript('features/example/Example.tsx', source).violations;
const css = (source, classes) => inspectCss('features/example/Example.module.css', source, classes);
const has = (items, code) => items.some((item) => item.code === code);

for (const source of [
  "import { Button } from '@mantine/core';",
  "import { Button } from 'react-aria-components';",
  "const library = import('react-aria-components/Menu');",
  "import '@mantine/core/styles.css';",
  "export { ReactFlow } from '@xyflow/react';",
  "const library = import('@mantine/core');",
  "const library = require('@xyflow/react');",
]) {
  test(`blocks direct UI library access: ${source}`, () =>
    assert.ok(has(script(source), 'library')));
}

test('Core may adapt external libraries; consumer semantic props and action spreads are allowed', () => {
  assert.deepEqual(
    inspectScript(
      'core/ui/Control.tsx',
      "import { Button } from '@mantine/core'; export const C = () => <Button styles={{root: {color: 'var(--text)'}}} />;",
    ).violations,
    [],
  );
  assert.deepEqual(
    script(
      "import { ActionButton as Command } from '../../core/ui/ActionButton'; const C = () => <Command tone='critical' density='compact' emphasis='outlined' {...actions}>Remove</Command>;",
    ),
    [],
  );
});

for (const source of [
  "import { ActionButton as Command } from '../../core/ui/ActionButton'; const C = () => <Command className='override' />;",
  "import { TextField } from '../../core/ui/TextField'; const C = () => <TextField classNames={{input: 'override'}} />;",
  "const C = () => <div style={{fontSize: 'var(--font-control)'}} />;",
  "import { ActionButton } from '../../core/ui/ActionButton'; const C = () => <ActionButton data-tone='critical' />;",
]) {
  test(`blocks caller appearance: ${source}`, () => assert.ok(has(script(source), 'override')));
}

for (const tag of ['button', 'input', 'select', 'textarea']) {
  test(`requires Core to own native ${tag}`, () =>
    assert.ok(has(script(`const C = () => <${tag} />;`), 'control')));
}

test('test fixtures may exercise native DOM, but stories follow consumer rules', () => {
  assert.deepEqual(
    inspectScript(
      'features/example/Example.test.tsx',
      "const fixture = <button style={{color:'#fff'}} />;",
    ).violations,
    [],
  );
  assert.ok(
    has(
      inspectScript('features/example/Example.stories.tsx', 'const C = () => <button />;')
        .violations,
      'control',
    ),
  );
});

test('inline story geometry and local layout variables do not require a visual token', () => {
  assert.deepEqual(
    script("const C = () => <div style={{height:'100dvh', minHeight:320, padding:24}} />;"),
    [],
  );
  assert.deepEqual(
    css(
      '.root { --layout-sidebar-width: 200px; display:grid; width:var(--layout-sidebar-width); color:var(--text); font-size:var(--font-body); }',
    ),
    [],
  );
});

test('Core components take no caller className, including former layout slots', () => {
  for (const source of [
    "import { ApplicationStatusBar as Header } from '../../core/ui/ApplicationStatusBar'; import s from './Example.module.css'; const C=()=> <Header className={s.toolbar} />;",
    "import { ApplicationStatusBar } from '../../core/ui/ApplicationStatusBar'; const C=()=> <ApplicationStatusBar className={dynamic} />;",
  ])
    assert.ok(has(script(source), 'override'));
  assert.deepEqual(
    script(
      "import { ApplicationStatusBar } from '../../core/ui/ApplicationStatusBar'; import s from './Example.module.css'; const C=()=> <div className={s.toolbar}><ApplicationStatusBar controls={null} /></div>;",
    ),
    [],
  );
});

for (const source of [
  '.root :global(.mantine-Button-label) { color:var(--red); }',
  '.root button { color:var(--text); }',
  ':root { --text:var(--red); }',
  '.wrapper [role="button"] { color:var(--red); }',
  '.root [data-variant="outline"] { border:0; }',
  '.root .action-button { padding:0; }',
  '.root * { font-size:var(--font-caption); }',
  '@import "./override.css";',
]) {
  test(`blocks consumer selector escapes: ${source}`, () =>
    assert.ok(has(css(source), 'selector')));
}

test('tokens cannot be locally redeclared or disguised as layout variables', () => {
  assert.ok(has(css('.root { --control-height:24px; }'), 'token'));
  assert.ok(has(css('.root { --special-button-color:var(--red); }'), 'token'));
  assert.ok(has(css('.root { --layout-color:red; color:var(--layout-color); }'), 'token'));
  assert.ok(has(css('.root { color:var(--text) !important; }'), 'important'));
});

test('typography and all color forms use system roles, including named colors', () => {
  for (const value of ['#fff', 'rgb(255 0 0)', 'oklch(50% 0.2 20)', 'rebeccapurple'])
    assert.ok(has(css(`.root { color:${value}; }`), 'color'));
  assert.ok(has(css('.root { font-size:12px; }'), 'typography'));
  assert.ok(has(css('.root { font-family:Arial; }'), 'typography'));
  for (const value of [
    '500 8px var(--font-mono)',
    'var(--font-label) Arial',
    '12px/1 "IBM Plex"',
    '500 var(--font-label, 12px) var(--font-sans)',
    'var(--font-label) var(--font-sans, Arial)',
    'var(--font-label, var(--font-body, 13px)) var(--font-sans)',
    'var(--font-label, ?) var(--font-sans)',
    'var(--font-label, {a:b}) var(--font-sans)',
  ])
    assert.ok(has(css(`.root { font:${value}; }`), 'typography'));
  assert.deepEqual(
    css(
      '.root { font:500 var(--font-label) / 1 var(--font-sans); } .b { font:inherit; } .c { font:italic var(--font-weight-medium) var(--font-body) / var(--line-height-body) var(--font-sans); } .d { font:revert; } .e { font:revert-layer; } .f { font:var(--font-label, var(--font-body)) var(--font-sans); }',
    ),
    [],
  );
  assert.deepEqual(
    css(
      '.root { color:currentColor; background:transparent; font-size:var(--font-control); font-family:inherit; }',
    ),
    [],
  );
  assert.ok(has(script("const color = '#fff';"), 'color'));
  assert.deepEqual(inspectScript('core/theme/tokens.ts', "const color = '#fff';").violations, []);
});

test('consumer imports cannot bypass Core by loading its stylesheet', () => {
  assert.ok(has(script("import '../../core/ui/ActionButton.module.css';"), 'selector'));
  assert.ok(has(script("import './override.css';"), 'selector'));
});

test('static object spreads cannot hide caller overrides, including aliases and nested spreads', () => {
  const source =
    "import { ActionButton } from '../../core/ui/ActionButton'; const base = {styles: {root: {color:'var(--red)'}}}; const alias = base; const props = {...alias, disabled:false}; const C=()=> <ActionButton {...props} />;";
  assert.ok(has(script(source), 'override'));
  assert.ok(
    has(script("const C=()=> <div {...{style:{fontSize:'var(--font-control)'}}} />;"), 'override'),
  );
  assert.deepEqual(
    script('const props = {disabled:true}; const C=()=> <Feature {...props} />;'),
    [],
  );
});

test('Core APIs cannot re-export the full library styling surface', () => {
  const broad =
    "import type {ButtonProps as LibraryProps} from '@mantine/core'; export function ActionButton(props: LibraryProps) { return null; }";
  assert.ok(has(inspectScript('core/ui/ActionButton.tsx', broad).violations, 'api'));
  const narrow =
    "import type {TextInputProps} from '@mantine/core'; type Props = Pick<TextInputProps, 'label' | 'onChange'>;";
  assert.deepEqual(inspectScript('core/ui/TextField.tsx', narrow).violations, []);
  assert.ok(
    has(
      inspectScript('core/ui/TextField.tsx', narrow.replace("'label'", "'styles'")).violations,
      'api',
    ),
  );
});

test('consumer JSX cannot inject global style sheets or replace a control render root', () => {
  assert.ok(has(script("const C=()=> <style>{'.action-button {padding:0}'}</style>;"), 'override'));
  assert.ok(
    has(
      script(
        "const C=()=> <div dangerouslySetInnerHTML={{__html:'<style>.action-button{padding:0}</style>'}} />;",
      ),
      'override',
    ),
  );
  assert.ok(has(script('const C=()=> <Feature renderRoot={customRoot} />;'), 'override'));
});

test('Core can declare bundled font faces while consumer typography still requires roles', () => {
  const face =
    "@font-face { font-family: 'IBM Plex Sans'; src: url('./fonts/Plex.woff2'); font-weight: 400; }";
  assert.deepEqual(inspectCss('core/theme/Fonts.css', face), []);
  assert.ok(has(css(face), 'typography'));
  assert.ok(has(inspectCss('core/theme/Bad.css', "p { font-family: 'Other'; }"), 'typography'));
});

test('spacing uses the shared scale in Core and consumers alike', () => {
  for (const rule of [
    'padding:3px',
    'padding:0 var(--space-2) 0 10px',
    'margin-top:-1px',
    'gap:4px 2px',
    'column-gap:0.5rem',
    'padding-inline:var(--row-inset, 9px)',
    'margin:var(--x, ?)',
    'grid-gap:9px',
    'grid-row-gap:2px',
    'PADDING:3px',
  ]) {
    assert.ok(has(css(`.root { ${rule}; }`), 'spacing'), rule);
    assert.ok(has(inspectCss('core/ui/Bad.module.css', `.root { ${rule}; }`), 'spacing'), rule);
  }
  assert.deepEqual(
    css(
      '.root { padding:0 var(--region-inset); margin:0 auto; gap:var(--space-1) var(--space-half); padding-block:calc((var(--size-md) - 1lh) / 2); margin-top:calc(-1 * var(--focus-width)); width:12px; }',
    ),
    [],
  );
});

test('stacking and motion use layer and motion tokens', () => {
  for (const rule of ['z-index:6', 'z-index:-1', 'z-index:var(--space-2)'])
    assert.ok(has(css(`.root { ${rule}; }`), 'layer'));
  for (const rule of [
    'transition:opacity 120ms ease',
    'transition-duration:0.2s',
    'animation:spin 1s linear infinite',
    'animation-delay:50ms',
    'transition:opacity var(--motion-duration-fast, 120ms) ease',
    'animation:spin var(--motion-duration-spin, 1s) linear infinite',
    'transition:opacity var(--motion-duration-fast, ?) ease',
  ])
    assert.ok(has(css(`.root { ${rule}; }`), 'motion'));
  assert.deepEqual(
    css(
      '.root { z-index:var(--layer-shell-chrome); transition:opacity var(--motion-duration-fast) var(--motion-easing-standard); } .b { z-index:auto; animation:spin var(--motion-duration-spin) linear infinite; }',
    ),
    [],
  );
});

test('every referenced variable is defined by tokens, CSS or inline style; tests are excluded', () => {
  const files = [
    { filename: 'core/theme/tokens.ts', source: "export const v = { '--text-primary': '#000' };" },
    { filename: 'core/ui/A.module.css', source: '.a { --local-gap: 2px; gap: var(--local-gap); }' },
    { filename: 'core/ui/B.tsx', source: "const s = { '--inline-x': '1px' };" },
    {
      filename: 'core/ui/C.module.css',
      source:
        '.c { color: var(--text-primary); width: var(--inline-x); max-width: var(--trigger-width); }',
    },
    {
      filename: 'core/ui/C.test.tsx',
      source: "const s = { '--muted': 'red' }; var(--only-in-test)",
    },
    { filename: 'features/D.module.css', source: '.d {\n  color: var(--muted);\n}' },
    {
      filename: 'core/ui/E.module.css',
      source:
        '@property --registered { syntax: "<length>"; inherits: false; initial-value: 0px; }\n/* --commented: 1px; */\n.e { width: var(--registered); height: var(--commented); }',
    },
    {
      filename: 'core/ui/F.tsx',
      source:
        "const a = { [`--template-key`]: '1px' }; el.style.setProperty('--set-property', '2px'); getComputedStyle(el).getPropertyValue('--read-only'); const b = 'var(--template-key) var(--set-property)';",
    },
    { filename: 'core/ui/G.module.css', source: '.g { width: var(--read-only); }' },
    {
      filename: 'core/ui/H.module.css',
      source: '.h { width: var(--text-primary, var(--fallback-missing)); }',
    },
    {
      filename: 'core/ui/I.module.css',
      source:
        '.i { --local: /* var(--commented-out) */ 1px; content: "var(--literal-text)"; width: var(--local); }',
    },
    {
      filename: 'core/ui/J.tsx',
      source:
        "el.style['setProperty']('--computed-method', '1px'); el.style?.setProperty('--optional-call', '1px'); other.setProperty('--not-style', '1px'); const c = 'var(--computed-method) var(--optional-call)';",
    },
    { filename: 'core/ui/K.module.css', source: '.k { width: var(--not-style); }' },
    {
      filename: 'core/ui/L.module.css',
      source:
        '.l { --data: { color: var(--block-missing); /* var(--block-comment) */ content: "var(--block-string)"; }; }',
    },
    {
      filename: 'core/ui/M.module.css',
      source: '.m { --data: { content: "/*"; color: var(--after-quoted-marker); /* end */ }; }',
    },
  ];
  const violations = inspectVariableReferences(files);
  assert.deepEqual(
    violations.map(({ filename, line, code }) => [filename, line, code]),
    [
      ['features/D.module.css', 2, 'token'],
      ['core/ui/E.module.css', 3, 'token'],
      ['core/ui/G.module.css', 1, 'token'],
      ['core/ui/H.module.css', 1, 'token'],
      ['core/ui/K.module.css', 1, 'token'],
      ['core/ui/L.module.css', 1, 'token'],
      ['core/ui/M.module.css', 1, 'token'],
    ],
  );
});

test('screen-level tests side-effect import the integration timeout owner; others need not', () => {
  const files = [
    { filename: 'screens/A.test.tsx', source: "import { StudioScreen } from './StudioScreen';" },
    {
      filename: 'screens/B.test.tsx',
      source:
        "import '../test-support/integration'; import { StudioScreen } from './StudioScreen';",
    },
    { filename: 'app/C.test.tsx', source: "import { App } from './App';" },
    {
      filename: 'features/D.test.tsx',
      source: "import type { RoutingWorkspace } from './RoutingWorkspace';",
    },
    { filename: 'core/ui/E.test.tsx', source: "import { ActionButton } from './ActionButton';" },
    {
      filename: 'features/F.test.tsx',
      source: "import { type RoutingWorkspace } from './RoutingWorkspace';",
    },
    {
      filename: 'screens/StudioScreen.tsx',
      source: "import { RoutingWorkspace } from '../x/RoutingWorkspace';",
    },
    {
      filename: 'screens/G.test.tsx',
      source:
        "import type { integrationTestTimeout } from '../test-support/integration'; import { StudioScreen } from './StudioScreen';",
    },
    { filename: 'app/H.test.tsx', source: "import * as AppModule from './App';" },
    {
      filename: 'screens/I.test.tsx',
      source: "import { MockStudioScreen } from './MockStudioScreen';",
    },
    {
      filename: 'screens/J.test.tsx',
      source:
        "import { integrationTestTimeout } from '../test-support/integration'; import { StudioScreen } from './StudioScreen';",
    },
  ];
  assert.deepEqual(
    inspectIntegrationTests(files).map(({ filename }) => filename),
    [
      'screens/A.test.tsx',
      'app/C.test.tsx',
      'screens/G.test.tsx',
      'app/H.test.tsx',
      'screens/J.test.tsx',
    ],
  );
});

test('only allowlisted tests render a whole screen, and the allowlist has no stale entry', () => {
  const allowed = new Set(['app/App.test.tsx', 'screens/Removed.test.tsx']);
  const files = [
    { filename: 'app/App.test.tsx', source: "import { App } from './App';" },
    {
      filename: 'screens/Logic.test.tsx',
      source: "import { x } from './y';\nimport { StudioScreen } from './StudioScreen';",
    },
    {
      filename: 'features/f/W.test.tsx',
      source: "import * as Workspace from '../../features/routing-workspace/RoutingWorkspace';",
    },
    { filename: 'screens/Rules.test.ts', source: "import { rule } from './studioRules';" },
    {
      filename: 'screens/Types.test.tsx',
      source: "import type { StudioScreen } from './StudioScreen';",
    },
    { filename: 'screens/StudioScreen.tsx', source: "import { App } from '../app/App';" },
    {
      filename: 'screens/Lazy.test.tsx',
      source: "it('x', async () => {\n  const { App } = await import('../app/App');\n});",
    },
  ];
  assert.deepEqual(
    inspectScreenRenderTests(files, allowed).map(({ filename, line, code }) => [
      filename,
      line,
      code,
    ]),
    [
      ['screens/Logic.test.tsx', 2, 'test'],
      ['features/f/W.test.tsx', 1, 'test'],
      ['screens/Lazy.test.tsx', 2, 'test'],
      ['screens/Removed.test.tsx', 1, 'test'],
    ],
  );
  assert.ok(screenRenderTests.has('screens/StudioScreen.test.tsx'));
});

const layer = (filename, source) =>
  inspectScript(filename, source).violations.filter((item) => item.code === 'layer');

test('domain imports only domain, and no package or Core view module', () => {
  assert.deepEqual(layer('core/domain/graph/topology.ts', "import { a } from './other';"), []);
  assert.deepEqual(layer('core/domain/graph/plan.ts', "import type { T } from '../model';"), []);
  for (const source of [
    "import { useState } from 'react';",
    "import type { Node } from '@xyflow/react';",
    "import { invoke } from '@tauri-apps/api/core';",
    "import { nodeWidths } from '../../theme/tokens';",
    "import type { StatusTone } from '../../ui/StatusBadge';",
    "import type { ControlPort } from '../../ports/control/ControlPort';",
    "export { x } from '../../adapters/x';",
    "type Port = import('../../ports/Clock').Clock;",
    "const later = import('../../ui/Knob');",
  ])
    assert.equal(layer('core/domain/graph/topology.ts', source).length, 1, source);
});

test('ports and application import only the layers inside them', () => {
  assert.deepEqual(
    layer(
      'core/ports/control/GraphPort.ts',
      "import type { G } from '../../domain/control/graph';",
    ),
    [],
  );
  assert.equal(
    layer('core/ports/Scheduler.ts', "import { T } from '../application/history/UndoHistory';")
      .length,
    1,
  );
  assert.deepEqual(
    layer(
      'core/application/history/UndoHistory.ts',
      "import type { Scheduler } from '../../ports/Scheduler'; import { x } from '../../domain/x';",
    ),
    [],
  );
  assert.equal(
    layer('core/application/telemetry/Hub.ts', "import { open } from '../../adapters/http';")
      .length,
    1,
  );
});

test('adapters may use platform packages but never the view', () => {
  assert.deepEqual(
    layer(
      'core/adapters/tauri/TauriNativeShell.ts',
      "import { invoke } from '@tauri-apps/api/core'; import type { P } from '../../ports/P'; import { H } from '../../application/telemetry/TelemetryHub';",
    ),
    [],
  );
  for (const source of [
    "import { useEffect } from 'react';",
    "import { useTranslation } from 'react-i18next';",
    "import { Knob } from '../../ui/Knob';",
    "import { App } from '../../../app/App';",
  ])
    assert.equal(layer('core/adapters/tauri/TauriNativeShell.ts', source).length, 1, source);
});

test('only composition roots, tests and stories choose adapters', () => {
  const adapter =
    "import { TauriControlPort } from '../../core/adapters/control/TauriControlPort';";
  assert.equal(layer('features/graphs/GraphList.tsx', adapter).length, 1);
  assert.deepEqual(layer('features/graphs/GraphList.test.tsx', adapter), []);
  assert.deepEqual(layer('features/graphs/GraphList.stories.tsx', adapter), []);
  assert.deepEqual(layer('screens/StudioScreen.tsx', adapter.replace('../../', '../')), []);
  assert.deepEqual(
    layer('app/localPorts.ts', "import { X } from '../core/adapters/tauri/TauriNativeShell';"),
    [],
  );
  assert.deepEqual(layer('main.tsx', "import { X } from './core/adapters/browser/storage';"), []);
  // Core view modules follow the same rule: only the ambient browser timers.
  assert.deepEqual(
    layer('core/history/useUndoHistory.ts', "import { t } from '../adapters/browser/timers';"),
    [],
  );
  for (const source of [
    "import { TauriControlPort } from '../adapters/control/TauriControlPort';",
    "import { s } from '../adapters/browser/themePreferenceStorage';",
    "export { s } from '../adapters/browser/languages';",
  ])
    assert.equal(layer('core/ui/Foo.tsx', source).length, 1, source);
  // Browser timers are the one ambient implementation features may default to.
  assert.deepEqual(
    layer(
      'features/a/Queue.ts',
      "import { timerScheduler } from '../../core/adapters/browser/timers';",
    ),
    [],
  );
  assert.equal(
    layer('features/a/Queue.ts', "import { s } from '../../core/adapters/browser/storage';").length,
    1,
  );
});

test('only adapters import platform APIs', () => {
  const platform = "import { invoke } from '@tauri-apps/api/core';";
  for (const filename of ['features/a/b.ts', 'core/ui/Foo.tsx', 'screens/S.tsx', 'app/App.tsx'])
    assert.equal(layer(filename, platform).length, 1, filename);
  assert.deepEqual(layer('core/adapters/tauri/TauriX.ts', platform), []);
  assert.deepEqual(layer('features/a/b.test.ts', platform), []);
});

test('test support is reachable from tests, stories and other test support only', () => {
  const fake = "import { fakeEngineFetch } from '../../test-support/fakeEngine';";
  for (const filename of ['features/a/b.ts', 'core/ui/Foo.tsx', 'core/domain/x.ts', 'app/App.tsx'])
    assert.equal(
      layer(filename, filename.startsWith('app/') ? fake.replace('../../', '../') : fake).length,
      1,
      filename,
    );
  assert.deepEqual(layer('features/a/b.test.ts', fake), []);
  assert.deepEqual(layer('features/a/b.stories.tsx', fake), []);
  assert.deepEqual(
    layer('test-support/fakeNativeShell.tsx', "import { x } from './fakeEngine';"),
    [],
  );
});

test('every import form is checked, including template literals and import-equals', () => {
  for (const source of [
    'const m = import(`../../ui/Knob`);',
    'import knob = require("../../ui/Knob");',
    'const m = require(`react`);',
  ])
    assert.equal(layer('core/domain/graph/topology.ts', source).length, 1, source);
  assert.equal(
    layer('features/a/b.ts', "const m = import.meta.glob('../../core/adapters/control/*.ts');")
      .length,
    1,
  );
  // A computed specifier cannot be resolved statically and is not reported.
  const computed = ['const m = import(`../$', '{name}`);'].join('');
  assert.deepEqual(layer('core/domain/graph/topology.ts', computed), []);
});

test('feature rules and application owners import only inward layers in their own feature', () => {
  for (const [filename, source] of [
    ['features/a/application/Owner.ts', "import { useState } from 'react';"],
    ['features/a/domain/rule.ts', "import type { Port } from '../ports/Port';"],
    ['features/a/ports/Port.ts', "import { Owner } from '../application/Owner';"],
    [
      'features/a/application/Owner.ts',
      "import { timers } from '../../../core/adapters/browser/timers';",
    ],
    ['features/a/application/Owner.ts', "import type { Props } from '../../../core/ui/Control';"],
    ['features/a/Hook.ts', "import { other } from '../b/model';"],
    ['features/a/Hook.test.tsx', "import { App } from '../../app/App';"],
    ['core/ui/Control.ts', "import type { Props } from '../../features/a/View';"],
  ])
    assert.ok(has(inspectScript(filename, source).violations, 'layer'), filename);
  assert.deepEqual(
    inspectScript(
      'features/a/application/Owner.ts',
      `
    import type { LocalPort } from '../ports/LocalPort';
    import { rule } from '../domain/rule';
    import type { Scheduler } from '../../../core/ports/Scheduler';
    import type { Reply } from '../../../core/domain/control/reply';
  `,
    ).violations,
    [],
  );
});

test('feature views and inner application owners receive time through ports', () => {
  for (const source of [
    'setTimeout(task, 10);',
    'window.setInterval(task, 10);',
    'clearTimeout(timer);',
    'globalThis.clearInterval(timer);',
    'const now = Date.now;',
    'performance.now();',
    'Date["now"]();',
  ])
    assert.ok(has(script(source), 'time'), source);
  assert.deepEqual(script('scheduler.after(10, task); clock.now();'), []);
  assert.deepEqual(
    inspectScript('features/a/Owner.test.ts', 'setTimeout(task, 10);').violations,
    [],
  );
  assert.deepEqual(
    inspectScript('core/adapters/browser/timers.ts', 'performance.now();').violations,
    [],
  );
});
