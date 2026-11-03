import { spawnSync } from 'node:child_process';
import { constants, copyFileSync, existsSync, readFileSync, readdirSync, lstatSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export function collectLinuxUiProblems(output, run = spawnSync) {
  const result = run('python3', [fileURLToPath(new URL('./linux-ui.py', import.meta.url)), output, '--diagnostics'], {
    encoding: 'utf8', timeout: process.env.MIXRELAY_E2E_VM_ACCELERATOR === 'tcg' ? 240000 : 60000,
    maxBuffer: 8 * 1024 * 1024,
  });
  writeFileSync(join(output, 'installed-ui-problems-command.log'),
    `${result.stdout ?? ''}\n${result.stderr ?? ''}\nexit=${result.status}; error=${result.error?.message ?? ''}\n`, { flag: 'wx' });
}

export function collectDesktopLogs(output, home = homedir()) {
  const directory = join(home, '.local/state/mixrelay/logs');
  if (!existsSync(directory)) return;
  for (const file of readdirSync(directory)) {
    if (!/^(desktop|driver-runtime)-\d+\.log$/.test(file)) continue;
    const source = join(directory, file);
    if (!lstatSync(source).isFile()) continue;
    copyFileSync(source, join(output, `app-log-${file}`), constants.COPYFILE_EXCL);
  }
}

export function collectLinux(output) {
  collectDesktopLogs(output);
  function capture(file, command, args, timeout = 15000) {
    const result = spawnSync(command, args, { encoding: 'utf8', timeout, maxBuffer: 8 * 1024 * 1024 });
    writeFileSync(join(output, file), `${result.stdout ?? ''}\n${result.stderr ?? ''}\nexit=${result.status}; error=${result.error?.message ?? ''}\n`, { flag: 'wx' });
  }
  capture('driver-journal.txt', 'journalctl', ['--user', '-u', 'mixrelay-driver.service', '--no-pager', '-n', '500']);
  capture('user-session-journal.txt', 'journalctl', ['--user', '--no-pager', '-n', '500']);
  capture('system-session-journal.txt', 'sudo', ['-n', 'journalctl', '--no-pager', '-n', '500']);
  capture('process-memory.txt', 'ps', ['-eo', 'pid,ppid,stat,pcpu,pmem,rss,comm,args']);
  capture('pipewire-state.json', 'pw-dump', []);
  for (const pid of readdirSync('/proc').filter(id => /^\d+$/.test(id))) {
    try {
      if (lstatSync(`/proc/${pid}`).uid !== process.getuid()) continue;
      const executable = readFileSync(`/proc/${pid}/cmdline`).toString().split('\0')[0];
      if (!['/usr/bin/mixrelay-desktop', '/usr/bin/mixrelay-service', '/usr/bin/mixrelay-driver-service'].includes(executable)) continue;
      capture(`${executable.split('/').at(-1)}-${pid}.stack.txt`, 'sudo', ['-n', 'python3',
        fileURLToPath(new URL('./linux-stack.py', import.meta.url)), pid]);
    } catch (error) { if (!['ENOENT', 'ESRCH'].includes(error.code)) throw error; }
  }
}
