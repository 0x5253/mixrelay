import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { arch, platform, release } from 'node:os';
import { digest } from '../support-contract.mjs';
import { parseOsRelease } from '../core/environment-policy.mjs';

const run = (program, args) => execFileSync(program, args, { encoding: 'utf8', timeout: 15000, maxBuffer: 8 * 1024 * 1024 }).trim();
function version(text) {
  const value = text.match(/\d+\.\d+(?:\.\d+)?/);
  if (!value) throw new Error('Dependency did not report a version');
  return value[0];
}

export function observeEnvironment() {
  const os = platform() === 'darwin' ? 'macos' : platform();
  const observed = { os, arch: arch(), kernel: release(), kind: 'native-session' };
  observed.desktopSession = { type: process.env.XDG_SESSION_TYPE ?? (os === 'macos' ? 'aqua' : 'unknown'),
    desktop: process.env.XDG_CURRENT_DESKTOP ?? null,
    display: process.env.WAYLAND_DISPLAY ? 'wayland' : process.env.DISPLAY ? 'x11' : null };
  if (os === 'macos') {
    observed.version = run('sw_vers', ['-productVersion']);
    observed.osRelease = run('sw_vers', []);
    observed.packageInventorySha256 = digest(run('pkgutil', ['--pkgs']));
    try { if (run('sysctl', ['-in', 'sysctl.proc_translated']) === '1') observed.translated = true; } catch {}
  } else if (os === 'linux') {
    observed.osRelease = readFileSync('/etc/os-release', 'utf8');
    const distro = parseOsRelease(observed.osRelease);
    observed.distro = distro.ID;
    observed.version = distro.VERSION_ID || distro.BUILD_ID || 'rolling';
    if (existsSync('/.dockerenv') || existsSync('/run/.containerenv')
      || existsSync('/run/systemd/container')) observed.kind = 'container';
    observed.dependencies = {
      pipewire: version(run('pipewire', ['--version'])),
      wireplumber: version(run('wireplumber', ['--version'])),
      glibc: version(run('getconf', ['GNU_LIBC_VERSION'])),
    };
    const packages = observed.distro === 'arch' ? run('pacman', ['-Q'])
      : observed.distro === 'fedora' ? run('rpm', ['-qa', '--queryformat', '%{NAME}\t%{VERSION}-%{RELEASE}\t%{ARCH}\n'])
        : run('dpkg-query', ['-W', '-f=${Package}\t${Version}\t${Architecture}\n']);
    observed.packageInventorySha256 = digest(packages);
  } else throw new Error(`Unsupported qualification host: ${os}`);
  return observed;
}
