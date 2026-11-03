import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  copyFileSync,
  createReadStream,
  createWriteStream,
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { createServer } from 'node:net';
import { arch, availableParallelism, platform } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { finished } from 'node:stream/promises';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { loadContract } from '../support-contract.mjs';

export function cloudConfig(publicKey, { repairEfiMount = false, recoverColdplug = false } = {}) {
  assert.match(publicKey, /^ssh-ed25519 [A-Za-z0-9+/=]+(?: [^\n]*)?$/);
  return (
    '#cloud-config\n' +
    JSON.stringify(
      {
        hostname: 'mixrelay-qa',
        disable_root: true,
        ...(recoverColdplug
          ? {
              bootcmd: [
                ['bash', '-c', readFileSync(new URL('./guest-boot-recovery.sh', import.meta.url), 'utf8')],
              ],
            }
          : {}),
        ...(repairEfiMount
          ? {
              bootcmd: [
                [
                  'bash',
                  '-c',
                  `set -euo pipefail
partuuid=$(blkid -s PARTUUID -o value /dev/vda15)
test -n "$partuuid"
cp /etc/fstab /etc/fstab.image-original
sed -i -E "s@^[^#[:space:]]+([[:space:]]+/boot/efi[[:space:]])@PARTUUID=$partuuid\\1@" /etc/fstab
systemctl daemon-reload
systemctl --no-block start default.target
`,
                ],
              ],
            }
          : {}),
        ssh_pwauth: false,
        // The guest keeps its pinned kernel and is discarded after qualification.
        // Ubuntu ARM needrestart can spend the entire install budget scanning vmlinuz.
        // Keep its library/service checks, but do not scan unused kernel images.
        write_files: [
          {
            path: '/etc/needrestart/conf.d/99-mixrelay-qualification.conf',
            permissions: '0644',
            content: '$nrconf{kernelhints} = 0;\n',
          },
        ],
        users: [
          {
            name: 'qa',
            shell: '/bin/bash',
            lock_passwd: true,
            sudo: 'ALL=(ALL) NOPASSWD:ALL',
            ssh_authorized_keys: [publicKey],
          },
        ],
      },
      null,
      2,
    ) +
    '\n'
  );
}

const guestMacAddress = '52:54:00:12:34:56';
export function guestCpuCount(accelerator, hostCpus) {
  assert.ok(['kvm', 'tcg'].includes(accelerator));
  assert.ok(Number.isInteger(hostCpus) && hostCpus > 0);
  return Math.min(hostCpus, accelerator === 'tcg' ? 4 : 2);
}

export function cloudNetwork() {
  return {
    version: 2,
    ethernets: { qa: { match: { macaddress: guestMacAddress }, dhcp4: true, dhcp6: false } },
  };
}

export function guestIdentity(env, accelerator) {
  assert.equal(env.GITHUB_ACTIONS, 'true');
  assert.ok(['kvm', 'tcg'].includes(accelerator));
  return ['GITHUB_ACTIONS', 'GITHUB_REPOSITORY', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_SHA']
    .map(name => {
      const value = env[name] ?? '';
      assert.match(value, /^[A-Za-z0-9/._-]+$/);
      return `${name}=${value}`;
    }).concat(`MIXRELAY_E2E_VM_ACCELERATOR=${accelerator}`,
      env.MIXRELAY_E2E_CONTROL_TRACE === 'true' ? ['MIXRELAY_E2E_CONTROL_TRACE=true'] : []);
}

export function qemuArguments({
  cpuArch,
  accelerator,
  disk,
  seed,
  firmware,
  variables,
  port,
  serial,
  cpus = 2,
}) {
  assert.ok(['x64', 'arm64'].includes(cpuArch));
  assert.ok(['kvm', 'tcg'].includes(accelerator));
  assert.ok(Number.isInteger(port) && port > 1024 && port < 65536);
  assert.ok(Number.isInteger(cpus) && cpus >= 1 && cpus <= 4);
  return [
    '-machine',
    `${cpuArch === 'x64' ? 'q35' : 'virt'},accel=${accelerator}`,
    '-cpu',
    accelerator === 'kvm' ? 'host' : cpuArch === 'arm64' ? 'cortex-a72' : 'max',
    '-smp',
    String(cpus),
    '-m',
    '4096',
    '-drive',
    `if=pflash,format=raw,readonly=on,file=${firmware}`,
    '-drive',
    `if=pflash,format=raw,file=${variables}`,
    '-drive',
    `if=virtio,format=qcow2,file=${disk}`,
    '-drive',
    `if=virtio,format=raw,readonly=on,file=${seed}`,
    '-netdev',
    `user,id=net0,hostfwd=tcp:127.0.0.1:${port}-:22`,
    '-device',
    `virtio-net-pci,netdev=net0,mac=${guestMacAddress},romfile=`,
    '-display',
    'none',
    '-serial',
    `file:${serial}`,
    '-monitor',
    'none',
    '-no-reboot',
  ];
}

export function guestDiskSize(imageInfo) {
  const size = imageInfo['virtual-size'];
  assert.ok(Number.isSafeInteger(size) && size > 0, 'Invalid cloud image virtual size');
  // Boot/EFI partitions can be at the end of a large sparse cloud disk.
  return Math.max(size, 20 * 1024 ** 3);
}

const activeCommands = new Set();
let interrupted = false;
export function command(program, args, { timeout = 60000, log, allowFailure = false, cwd, progress } = {}) {
  assert.equal(interrupted, false, 'VM job was interrupted');
  return new Promise((resolvePromise, reject) => {
    const child = spawn(program, args, { cwd, stdio: ['ignore', 'pipe', 'pipe'] });
    activeCommands.add(child);
    const stream = log && createWriteStream(log, { flags: 'a' });
    // Observe write errors immediately and retain the flush result until the child closes.
    const logFinished = stream ? finished(stream).then(() => null, error => {
      child.kill('SIGKILL');
      return error;
    }) : Promise.resolve(null);
    let output = '',
      settled = false;
    for (const pipe of [child.stdout, child.stderr])
      pipe.on('data', (bytes) => {
        stream?.write(bytes);
        progress?.(bytes);
        if (output.length < 1024 * 1024) output += bytes.toString();
      });
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
    }, timeout);
    const complete = (error, code) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      stream?.end();
      activeCommands.delete(child);
      // Child close drains its pipes; the evidence file has a separate flush lifetime.
      logFinished.then(logError => {
        if (interrupted) reject(new Error('VM job was interrupted'));
        else if (error || logError) reject(error ?? logError);
        else if (code !== 0 && !allowFailure)
          reject(new Error(`${program} exited ${code}; ${log ? `see ${log}` : output.slice(-2000)}`));
        else resolvePromise({ code, output: output.trim() });
      });
    };
    child.once('error', (error) => complete(error));
    child.once('close', (code) => complete(null, code));
  });
}

async function downloadImage(image, destination) {
  await command(
    'curl',
    [
      '--fail',
      '--location',
      '--retry',
      '3',
      '--silent',
      '--show-error',
      '--output',
      destination,
      image.url,
    ],
    { timeout: 600000 },
  );
  const hash = createHash(image.algorithm);
  for await (const bytes of createReadStream(destination)) hash.update(bytes);
  assert.equal(hash.digest('hex'), image.digest, 'Cloud image checksum mismatch');
}

async function unusedPort() {
  const server = createServer();
  await new Promise((resolvePromise, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolvePromise);
  });
  const port = server.address().port;
  await new Promise((resolvePromise) => server.close(resolvePromise));
  return port;
}

export async function copyCandidates(command, sshOptions, port, directory) {
  const files = readdirSync(directory).map((file) => join(directory, file));
  return command(
    'scp',
    [...sshOptions, '-P', String(port), '-r', ...files, 'qa@127.0.0.1:/home/qa/candidate/'],
    { timeout: 300000 },
  );
}

// The host runs QEMU; the app and PipeWire run in the guest's complete systemd OS.
export async function runGithubVm({ targetId, directory, output, smoke = false }) {
  assert.equal(platform(), 'linux');
  assert.equal(
    process.env.GITHUB_ACTIONS,
    'true',
    'This adapter runs only on disposable GitHub-hosted Linux runners',
  );
  assert.equal(process.env.RUNNER_ENVIRONMENT, 'github-hosted');
  const { contract } = loadContract();
  const target = contract.qualificationTargets.find((t) => t.id === targetId);
  assert.equal(target?.execution.kind, 'linux-vm');
  assert.equal(arch(), target.arch, 'Use a hosted runner with the guest CPU architecture');
  const image = contract.qualificationImages[target.execution.image];
  const state = resolve('.lab', `qa-vm-${targetId}`);
  assert.ok(!existsSync(state), 'VM state directory already exists');
  assert.ok(!existsSync(output), 'Evidence directory already exists');
  mkdirSync(state, { recursive: true, mode: 0o700 });
  mkdirSync(output, { recursive: false });
  let qemu,
    session = {
      targetId,
      scope: smoke ? 'host-readiness' : 'installed-qualification',
      image,
      arch: arch(),
    };
  const serial = join(output, 'vm-serial.log');
  const stop = () => {
    if (qemu?.exitCode === null && qemu?.signalCode === null) qemu.kill('SIGTERM');
  };
  const cancelled = () => {
    interrupted = true;
    stop();
    for (const child of activeCommands) child.kill('SIGKILL');
    process.exitCode = 1;
  };
  process.once('SIGTERM', cancelled);
  process.once('SIGINT', cancelled);
  const phase = (message) => console.log(`${targetId}: ${message}`);
  try {
    const archive = join(state, 'image.download');
    phase('Downloading and verifying the pinned cloud image');
    await downloadImage(image, archive);
    let base = archive;
    if (image.format === 'tar.xz') {
      const { output: listing } = await command('tar', ['-tf', archive], { timeout: 300000 });
      const disks = listing.split('\n').filter((path) => /(?:^|\/)disk\.raw$/.test(path));
      assert.equal(disks.length, 1, 'Expected one disk.raw in the cloud image');
      assert.ok(!disks[0].split('/').includes('..') && !disks[0].startsWith('/'));
      // Extract only the pinned disk and preserve sparse extents on the hosted SSD.
      await command('tar', ['-xSf', archive, '--no-same-owner', '-C', state, '--', disks[0]], {
        timeout: 300000,
      });
      base = join(state, disks[0]);
      assert.ok(lstatSync(base).isFile(), 'Cloud disk must be a regular file');
    }
    const info = JSON.parse((await command('qemu-img', ['info', '--output=json', base])).output);
    assert.equal(info.format, image.format === 'tar.xz' ? 'raw' : 'qcow2');
    assert.ok(!info['backing-filename'], 'Cloud base image must be self-contained');
    const disk = join(state, 'guest.qcow2');
    session.baseVirtualSize = info['virtual-size'];
    session.guestVirtualSize = guestDiskSize(info);
    await command('qemu-img', [
      'create',
      '-f',
      'qcow2',
      '-F',
      info.format,
      '-b',
      base,
      disk,
      String(session.guestVirtualSize),
    ]);
    const key = join(state, 'ssh');
    await command('ssh-keygen', ['-t', 'ed25519', '-N', '', '-f', key, '-q']);
    // The pinned Kali ARM image has a stale EFI PARTUUID in fstab. Preserve the
    // image entry and repair only that mount in the disposable guest overlay.
    const repairEfiMount = targetId === 'kali-rolling-arm64';
    const recoverColdplug = targetId === 'ubuntu-26-04-arm64';
    session.provisioning = { repairEfiMount, recoverColdplug, needrestartKernelHints: false };
    writeFileSync(
      join(state, 'user-data'),
      cloudConfig(readFileSync(`${key}.pub`, 'utf8').trim(), { repairEfiMount, recoverColdplug }),
    );
    writeFileSync(
      join(state, 'meta-data'),
      `instance-id: mixrelay-${targetId}\nlocal-hostname: mixrelay-qa\n`,
    );
    writeFileSync(join(state, 'network-config'), `${JSON.stringify(cloudNetwork(), null, 2)}\n`);
    const seed = join(state, 'seed.img');
    await command('cloud-localds', [
      '--network-config',
      join(state, 'network-config'),
      seed,
      join(state, 'user-data'),
      join(state, 'meta-data'),
    ]);
    const firmware =
      arch() === 'arm64' ? '/usr/share/AAVMF/AAVMF_CODE.fd' : '/usr/share/OVMF/OVMF_CODE_4M.fd';
    const originalVariables =
      arch() === 'arm64' ? '/usr/share/AAVMF/AAVMF_VARS.fd' : '/usr/share/OVMF/OVMF_VARS_4M.fd';
    assert.ok(existsSync(firmware) && existsSync(originalVariables), 'Missing UEFI firmware');
    const variables = join(state, 'uefi-vars.fd');
    copyFileSync(originalVariables, variables);
    const port = await unusedPort();
    // KVM permissions are configured in the workflow; ARM hosts may expose no KVM.
    const kvm = await command('test', ['-r', '/dev/kvm'], { allowFailure: true });
    const writable = await command('test', ['-w', '/dev/kvm'], { allowFailure: true });
    session.accelerator = kvm.code === 0 && writable.code === 0 ? 'kvm' : 'tcg';
    session.cpuModel =
      session.accelerator === 'kvm' ? 'host' : target.arch === 'arm64' ? 'cortex-a72' : 'max';
    session.hostCpuCount = availableParallelism();
    session.guestCpuCount = guestCpuCount(session.accelerator, session.hostCpuCount);
    phase(`Booting ${target.arch} guest with ${session.accelerator}; waiting for SSH`);
    qemu = spawn(
      arch() === 'arm64' ? 'qemu-system-aarch64' : 'qemu-system-x86_64',
      qemuArguments({
        cpuArch: arch(),
        accelerator: session.accelerator,
        disk,
        seed,
        firmware,
        variables,
        port,
        serial,
        cpus: session.guestCpuCount,
      }),
      { stdio: ['ignore', 'ignore', 'pipe'] },
    );
    qemu.stderr.pipe(createWriteStream(join(output, 'qemu.log')));
    let bootError;
    qemu.on('error', (error) => {
      bootError = error;
    });
    const sshOptions = [
      '-i',
      key,
      '-o',
      'BatchMode=yes',
      '-o',
      'StrictHostKeyChecking=accept-new',
      '-o',
      `UserKnownHostsFile=${join(state, 'known_hosts')}`,
      '-o',
      'ConnectTimeout=5',
    ];
    const ssh = (args, options) =>
      command('ssh', [...sshOptions, '-p', String(port), 'qa@127.0.0.1', ...args], options);
    const deadline = Date.now() + 600000;
    while (true) {
      if (bootError) throw bootError;
      assert.ok(
        qemu.exitCode === null && qemu.signalCode === null,
        'QEMU stopped during boot; see serial log',
      );
      if ((await ssh(['true'], { allowFailure: true, timeout: 10000 })).code === 0) break;
      assert.ok(Date.now() < deadline, 'Guest SSH readiness timed out');
      await delay(2000);
    }
    const bootstrapLog = join(output, 'guest-bootstrap.log');
    phase('SSH is ready; waiting for cloud-init');
    await ssh(['sudo cloud-init status --wait'], { timeout: 180000, log: bootstrapLog });
    await ssh(['mkdir -p /home/qa/workspace /home/qa/tools /home/qa/candidate']);
    const source = join(state, 'source.tar');
    await command('git', ['archive', '--format=tar', '-o', source, 'HEAD']);
    await command('scp', [
      ...sshOptions,
      '-P',
      String(port),
      source,
      'qa@127.0.0.1:/home/qa/source.tar',
    ]);
    await command('scp', [
      ...sshOptions,
      '-P',
      String(port),
      process.execPath,
      'qa@127.0.0.1:/home/qa/tools/node',
    ]);
    await ssh([
      'tar -xf /home/qa/source.tar -C /home/qa/workspace && chmod +x /home/qa/tools/node',
    ]);
    phase('Preparing the guest desktop and PipeWire user session');
    await ssh(['sudo bash /home/qa/workspace/scripts/e2e/guest-bootstrap.sh'], {
      timeout: 2100000,
      log: bootstrapLog,
    });
    if (!smoke) {
      await copyCandidates(command, sshOptions, port, directory);
    }
    // Forward only public run identity. Workflow tokens and host environment stay outside the VM.
    const identity = guestIdentity(process.env, session.accelerator);
    phase(
      smoke
        ? 'Checking the actual OS and desktop audio prerequisites'
        : 'Installing the exact candidate and running the shared suite',
    );
    const traceSignals = !smoke && target.id === 'arch-rolling-x64';
    const traceCommand = action =>
      `sudo -n python3 /home/qa/workspace/scripts/e2e/guest-signal-trace.py ${action} --disposable-environment`;
    if (traceSignals) {
      await ssh([traceCommand('start')], { log: join(output, 'guest-signal-trace-setup.log') });
    }
    const result = await ssh(
      [
        `env ${identity.join(' ')} bash /home/qa/workspace/scripts/e2e/guest-session.sh ${smoke ? '--smoke ' : ''}${target.id}`,
      ],
      { timeout: 5400000, allowFailure: true, log: join(output, 'guest-suite.log'),
        progress: bytes => process.stdout.write(bytes) },
    );
    const traceStopped = traceSignals
      ? await ssh([traceCommand('stop')], { allowFailure: true, log: join(output, 'guest-signal-trace.log') })
      : { code: 0 };
    const collected = await command(
      'scp',
      [
        ...sshOptions,
        '-P',
        String(port),
        '-r',
        'qa@127.0.0.1:/home/qa/workspace/evidence/.',
        output,
      ],
      { timeout: 180000, allowFailure: true },
    );
    session.guestExitCode = result.code;
    session.evidenceCollected = collected.code === 0;
    assert.equal(
      result.code,
      0,
      'Guest suite failed; see guest-suite.log and qualification report',
    );
    assert.equal(collected.code, 0, 'Guest evidence collection failed');
    assert.equal(traceStopped.code, 0, 'Guest signal trace cleanup failed; see guest-signal-trace.log');
    return session;
  } catch (error) {
    session.error = error.message;
    throw error;
  } finally {
    stop();
    if (qemu?.exitCode === null && qemu?.signalCode === null) {
      await Promise.race([
        new Promise((resolvePromise) => qemu.once('close', resolvePromise)),
        delay(5000),
      ]);
      if (qemu.exitCode === null && qemu.signalCode === null) qemu.kill('SIGKILL');
    }
    writeFileSync(join(output, 'vm-session.json'), `${JSON.stringify(session, null, 2)}\n`);
    rmSync(state, { recursive: true, force: true });
    process.removeListener('SIGTERM', cancelled);
    process.removeListener('SIGINT', cancelled);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values } = parseArgs({
    options: {
      target: { type: 'string' },
      directory: { type: 'string' },
      output: { type: 'string' },
      smoke: { type: 'boolean', default: false },
    },
  });
  runGithubVm({
    targetId: values.target,
    directory: values.directory && resolve(values.directory),
    output: resolve(values.output),
    smoke: values.smoke,
  }).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
