const publicHosts = new Set([
  'rawsector.com', 'appserver.rawsector.com', 'downloads.rawsector.com',
  'support.apple.com', 'law.go.kr', 'www.law.go.kr', 'www.easylaw.go.kr',
]);
const publicEmails = new Set([
  'support@rawsector.com', 'privacy@dsub.io', 'legal@dsub.io', 'giovanni@fragcolor.com',
]);
// Existing third-party licence references retain their original protocols.
const licenceDestinations = new Set([
  'http://scripts.sil.org/OFL', 'https://openfontlicense.org',
  'http://www.apache.org/licenses/LICENSE-2.0', 'http://www.apache.org/licenses',
  'https://github.com/alsa-project/alsa-lib',
]);

export function assertPublicDestination(destination, surface = 'public surface') {
  const fail = () => { throw new Error(`${surface}: unapproved public destination: ${destination}`); };
  if (destination.startsWith('mailto:') || !destination.includes('://') && !destination.startsWith('//')) {
    const email = destination.replace(/^mailto:/, '').split('?', 1)[0].toLowerCase();
    if (!publicEmails.has(email)) fail();
    return;
  }
  let url;
  try { url = new URL(destination.startsWith('//') ? `https:${destination}` : destination); }
  catch { fail(); }
  if (url.username || url.password || url.port) fail();
  if (licenceDestinations.has(destination.replace(/\/$/, ''))) return;
  const version = '(?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*)';
  const latestRelease = url.hostname === 'api.github.com'
    && url.pathname === '/repos/0x5253/mixrelay/releases/latest' && !url.search && !url.hash;
  let sourceRepository = url.hostname === 'github.com' && /^\/0x5253\/mixrelay(?:\/|$)/.test(url.pathname);
  if (sourceRepository && url.pathname.startsWith('/0x5253/mixrelay/releases/download/')) {
    const release = url.pathname.match(new RegExp(`^/0x5253/mixrelay/releases/download/v(${version})/([^/]+)$`));
    const releaseVersion = release?.[1];
    const assets = releaseVersion ? [
      `MixRelay-Installer-${releaseVersion}.pkg`, `MixRelay_${releaseVersion}_amd64.deb`,
      `MixRelay_${releaseVersion}_arm64.deb`, `mix-relay-${releaseVersion}-1-x86_64.pkg.tar.zst`,
    ] : [];
    sourceRepository = assets.includes(release?.[2]) && !url.search && !url.hash;
  }
  if (url.protocol !== 'https:' || (!publicHosts.has(url.hostname) && !sourceRepository && !latestRelease)) fail();
  if (/\.streamDeckPlugin(?:$|[?#])/i.test(url.pathname)) fail();
}

export function extractDestinations(source) {
  const text = source.replaceAll('&amp;', '&').replaceAll('&#64;', '@')
    .replaceAll('&#58;', ':').replaceAll('&#47;', '/');
  const urls = [...text.matchAll(/(?:[a-z][a-z\d+.-]*:\/\/|\/\/)[a-z\d][^\s<>"'`\\{}\u0080-\uffff]*/gi)]
    .map(match => match[0].replace(/[),.;\]]+$/, ''));
  const emails = [...text.matchAll(/(?<![\w/])[a-z\d._%+-]+@[a-z\d](?:[a-z\d.-]*[a-z\d])?\.[a-z]{2,}/gi)]
    .map(match => match[0]);
  return [...new Set([...urls, ...emails])];
}

export function assertPublicReferences(source, surface) {
  for (const destination of extractDestinations(source)) assertPublicDestination(destination, surface);
  if (surface === 'README.md' && /\.streamDeckPlugin/i.test(source)) {
    throw new Error(`${surface}: customer plug-in installation must use Marketplace`);
  }
  if (/\b(?:mixrelay\.com\b|Unknown Packager\b)/i.test(source)) {
    throw new Error(`${surface}: unresolved or anonymous public reference`);
  }
}

export function isUserFacingSurface(path) {
  if (/(?:^|\/)(?:tests?|test-support|fixtures?|__tests__|node_modules|target|dist|storybook)(?:\/|[._-])|\.(?:test|stories|spec)\.|(?:^|_)tests?\.rs$/.test(path)) return false;
  if (['README.md', 'Cargo.toml'].includes(path) || /^LICENSE.*\.txt$/.test(path)) return true;
  if (/^(?:apps\/[^/]+\/)?package\.json$/.test(path)) return true;
  if (/^apps\/website\//.test(path)) return !/\/fonts\/sources\.json$/.test(path) && /\.(?:html|css|js|json|txt)$/.test(path) && !/check-site\.mjs$/.test(path);
  if (/^apps\/control-ui\/src\//.test(path)) return /\.(?:ts|tsx|css|txt)$/.test(path);
  if (/^apps\/desktop\/src-tauri\//.test(path)) return /(?:src\/.*\.(?:rs|m)|Cargo\.toml|tauri[^/]*\.json|Info\.plist)$/.test(path);
  if (/^native\//.test(path)) return /\.(?:rs|m|mm|c|h|plist|strings)$/.test(path);
  if (/^packaging\//.test(path)) return !/\.md$/.test(path);
  if (/^apps\/streamdeck-plugin\//.test(path)) return /\.(?:json|html|mjs|css)$/.test(path);
  if (/^(?:third-party\/|vendor\/LICENSE)/.test(path)) return /(?:LICENSE[^/]*|COPYING|NOTICE)$/.test(path);
  if (/^scripts\/(?:build-macos-installer-pkgs|build-macos-removal-app|prepare-macos-release-payload|prepare-release-manifest)\.mjs$/.test(path)) return true;
  return /(?:^|\/)[^/]+\.(?:release|update)\.json$/.test(path) && !/^(?:docs|\.github|\.lab)\//.test(path);
}

export function publicSurfaceText(source, path) {
  let text = source.replace(/<!DOCTYPE[^>]*>/g, '').replace(/\bxmlns(?::\w+)?="[^"]*"/g, '')
    .replace(/"\$schema"\s*:\s*"[^"]*"/g, '');
  if (/\.(?:rs|m|mm|c|h|ts|tsx|mjs|js|css)$/.test(path)) {
    text = text.replace(/^#\[cfg\(test\)\]\s*\nmod tests \{[\s\S]*?^}/gm, '')
      .replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
  }
  // Development transport addresses are not customer destinations.
  if (/tauri[^/]*\.json$/.test(path)) text = text.replace(/"devUrl"\s*:\s*"[^"]*"/g, '');
  return text.replaceAll('ws://127.0.0.1:*', '');
}
