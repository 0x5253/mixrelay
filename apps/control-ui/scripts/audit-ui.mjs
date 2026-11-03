import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from '@babel/parser';

const project = fileURLToPath(new URL('../', import.meta.url));
const controls = new Set([
  'button',
  'input',
  'select',
  'textarea',
  'Button',
  'NumberField',
  'Slider',
  'ActionButton',
  'AppInformationButton',
  'CheckboxField',
  'CollectionItem',
  'GraphicEqualizer',
  'HierarchicalMenuButton',
  'IconActionButton',
  'InfoPopover',
  'InlineSelect',
  'Knob',
  'LevelControl',
  'NativeSampleFormatField',
  'NodeMuteButton',
  'NodeOptionsSection',
  'NodePowerButton',
  'NumericValueControl',
  'ParameterField',
  'PathField',
  'RecoveryActions',
  'RetryAlert',
  'SelectField',
  'SidebarToggle',
  'StateButton',
  'TextField',
  'ThemePicker',
  'TitleField',
  'ToggleField',
  'TransportButton',
  'ValueSlider',
]);

function walk(node, visit, owner = 'render adapter') {
  if (!node || typeof node !== 'object') return;
  const nextOwner = node.type === 'FunctionDeclaration' ? (node.id?.name ?? owner) : owner;
  visit(node, nextOwner);
  for (const [key, value] of Object.entries(node)) {
    if (key === 'loc') continue;
    if (Array.isArray(value))
      value.forEach((child) => {
        walk(child, visit, nextOwner);
      });
    else if (value && typeof value === 'object') walk(value, visit, nextOwner);
  }
}

function nativeAlias(node) {
  if (node?.type === 'StringLiteral') return node.value;
  if (node?.type === 'ConditionalExpression') {
    return [nativeAlias(node.consequent), nativeAlias(node.alternate)].filter(Boolean).join('|');
  }
  return null;
}

export function interactionSites(source) {
  const ast = parse(source, { sourceType: 'module', plugins: ['typescript', 'jsx'] });
  const aliases = new Map();
  walk(ast, (node) => {
    if (node.type === 'ImportSpecifier') aliases.set(node.local.name, node.imported.name);
    if (node.type === 'VariableDeclarator' && node.id.type === 'Identifier') {
      const tag = nativeAlias(node.init);
      if (tag) aliases.set(node.id.name, tag);
    }
  });
  const sites = [];
  walk(ast, (node, owner) => {
    if (node.type !== 'JSXOpeningElement') return;
    const tag = node.name.name;
    const resolved = aliases.get(tag) ?? tag;
    const attrs = node.attributes.filter((attr) => attr.type === 'JSXAttribute');
    const values = Object.fromEntries(
      attrs.map((attr) => [
        attr.name.name,
        attr.value ? source.slice(attr.value.start, attr.value.end) : 'true',
      ]),
    );
    if (!resolved?.split('|').some((name) => controls.has(name)) && values.role !== '"button"')
      return;
    sites.push({
      line: node.loc.start.line,
      owner,
      tag,
      resolved,
      name:
        values['aria-label'] ?? values['aria-labelledby'] ?? values.label ?? values.title ?? null,
      inherited: node.attributes.some((attr) => attr.type === 'JSXSpreadAttribute'),
      hidden: values['aria-hidden'] ?? null,
    });
  });
  return sites;
}

function files(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const file = path.join(directory, entry.name);
    return entry.isDirectory() ? files(file) : [file];
  });
}

function inventory() {
  const sources = files(path.join(project, 'src')).filter(
    (file) => file.endsWith('.tsx') && !/\.(test|stories)\.tsx$|\/design-system\//u.test(file),
  );
  const sites = sources.flatMap((file) =>
    interactionSites(fs.readFileSync(file, 'utf8')).map((site) => ({
      file: path.relative(project, file),
      ...site,
    })),
  );
  return { sourceFiles: sources.length, sites };
}

function main(args) {
  if (args.includes('--help')) {
    console.log(
      'Usage: pnpm --filter @mixrelay/control-ui audit:ui [--json]\n' +
        'Lists interactive source sites, native aliases and accessible-name expressions.\n' +
        'Default: readable inventory. --json: JSON on stdout; no files are created.\n' +
        'Missing expressions may come from visible children or props; review in Storybook.\n' +
        'This inventory does not verify computed names or keyboard behavior.\n' +
        'Unknown options or source parsing errors exit with a nonzero status.',
    );
    return;
  }
  if (args.some((arg) => arg !== '--json')) throw new Error('Unknown option. Use --help.');
  const result = inventory();
  if (args.includes('--json')) {
    console.log(JSON.stringify(result, null, 2));
    return;
  }
  console.log(
    `${result.sourceFiles} source files; ${result.sites.length} interactive sites.\n` +
      'Static review inventory; confirm runtime names and keys in Storybook.',
  );
  for (const site of result.sites) {
    const name =
      site.name?.replace(/\s+/gu, ' ').slice(0, 110) ?? '(visible text / inherited props)';
    console.log(`${site.file}:${site.line}  ${site.owner} / ${site.resolved}  ${name}`);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2));
}
