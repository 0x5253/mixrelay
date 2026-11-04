import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { cloudConfig, cloudNetwork, command, guestCpuCount, guestDiskSize, guestIdentity, qemuArguments } from './github-vm.mjs';

test('disposable guest diagnostics receive CI identity without workflow credentials or shell input', () => {
  const env = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: '0x5253/mixrelay', GITHUB_RUN_ID: '42',
    GITHUB_RUN_ATTEMPT: '1', GITHUB_SHA: 'a'.repeat(40), MIXRELAY_E2E_CONTROL_TRACE: 'true', GH_TOKEN: 'private' };
  const identity = guestIdentity(env, 'tcg');
  assert.ok(identity.includes('GITHUB_ACTIONS=true'));
  assert.ok(identity.includes('MIXRELAY_E2E_CONTROL_TRACE=true'));
  assert.ok(identity.includes('MIXRELAY_E2E_VM_ACCELERATOR=tcg'));
  assert.equal(identity.some(value => value.includes('private') || value.includes('GH_TOKEN')), false);
  assert.throws(() => guestIdentity({ ...env, GITHUB_SHA: 'head; command' }, 'tcg'));
  assert.throws(() => guestIdentity({ ...env, GITHUB_ACTIONS: undefined }, 'tcg'));
});

test('guest progress reaches the live console and raw evidence without masking a nonzero exit', async t => {
  const { mkdtempSync, readFileSync, rmSync } = await import('node:fs');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const directory = mkdtempSync(join(tmpdir(), 'mr-guest-progress-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const log = join(directory, 'suite.log');
  let progress = '';
  const result = await command(process.execPath, ['-e',
    "process.stdout.write('AUDIO-01: started\\n'); process.stderr.write('native failure\\n'); process.exitCode = 7;"],
  { log, allowFailure: true, progress: bytes => { progress += bytes; } });
  assert.equal(result.code, 7);
  assert.match(progress, /AUDIO-01: started\n/);
  assert.match(progress, /native failure\n/);
  assert.equal(readFileSync(log, 'utf8'), progress);
  await assert.rejects(command(process.execPath, ['-e', 'process.exitCode = 7']), /exited 7/);
  await assert.rejects(command(process.execPath, ['-e', "process.stdout.write('evidence');"],
    { log: directory, allowFailure: true }), error => error.code === 'EISDIR');
});

test('cloud guest has a locked account and a job-specific SSH key', () => {
  const config = JSON.parse(cloudConfig('ssh-ed25519 AAAA test').split('\n').slice(1).join('\n'));
  assert.equal(config.ssh_pwauth, false);
  assert.equal(config.disable_root, true);
  assert.equal(config.users[0].lock_passwd, true);
  assert.deepEqual(config.users[0].ssh_authorized_keys, ['ssh-ed25519 AAAA test']);
  assert.throws(() => cloudConfig('ssh-ed25519 AAAA\nusers: bad'));
});

test('cloud overlays preserve large disks with trailing boot partitions and grow small disks', () => {
  assert.equal(guestDiskSize({ 'virtual-size': 3 * 1024 ** 3 }), 20 * 1024 ** 3);
  assert.equal(guestDiskSize({ 'virtual-size': 32 * 1024 ** 3 }), 32 * 1024 ** 3);
  assert.throws(() => guestDiskSize({ 'virtual-size': -1 }));
});

function bootRecovery(environment = {}) {
  const config = JSON.parse(
    cloudConfig('ssh-ed25519 AAAA test', { recoverColdplug: true }).split('\n').slice(1).join('\n'),
  );
  assert.equal(config.bootcmd.length, 1);
  return execFileSync(
    'bash',
    [
      '-c',
      `
systemctl() {
  printf 'systemctl %s\\n' "$*"
  if [[ "$*" == '--quiet is-active emergency.target' ]]; then
    return "\${BOOT_EMERGENCY:-0}"
  fi
  if [[ "$*" == 'start boot.mount boot-efi.mount' ]]; then
    return "\${BOOT_MOUNT_FAILURE:-0}"
  fi
}
udevadm() { printf 'udevadm %s\\n' "$*"; }
blkid() {
  if [[ "$2" == "$BOOT_MISSING_DEVICE" ]]; then return 2; fi
  printf '/dev/%s\\n' "$2"
}
findmnt() { printf 'findmnt %s\\n' "$*"; }
export -f systemctl udevadm blkid findmnt
"$@"
`,
      '--',
      ...config.bootcmd[0],
    ],
    { encoding: 'utf8', env: { ...process.env, BOOT_MISSING_DEVICE: '', ...environment } },
  );
}

test('emergency boot waits for coldplug and retries both original mount units before default', () => {
  const output = bootRecovery();
  assert.ok(output.includes('BOOT: /dev/BOOT'));
  assert.ok(output.includes('UEFI: /dev/UEFI'));
  assert.ok(output.indexOf('udevadm settle') < output.indexOf('systemctl start boot.mount'));
  assert.ok(output.indexOf('findmnt --mountpoint /boot/efi') < output.indexOf('systemctl --no-block default'));
  assert.ok(output.includes('systemctl start boot.mount boot-efi.mount'));
});

test('recovered emergency boot requeues cloud-init configuration and finalization', () => {
  const output = bootRecovery();
  const mounted = output.indexOf('findmnt --mountpoint /boot/efi');
  const normalBoot = output.indexOf('systemctl --no-block default');
  const resumed = output.indexOf('systemctl --no-block start cloud-config.service cloud-final.service');
  assert.ok(normalBoot > mounted, 'Normal boot must resume only after both original mounts are present');
  assert.ok(resumed > normalBoot, 'The interrupted cloud-init stages must be queued after normal boot resumes');
});

test('healthy boot needs no mount recovery and a failed mount never starts default', () => {
  const healthy = bootRecovery({ BOOT_EMERGENCY: '3' });
  assert.equal(healthy.trim(), 'systemctl --quiet is-active emergency.target');
  assert.throws(
    () => bootRecovery({ BOOT_MOUNT_FAILURE: '73' }),
    (error) => {
      assert.equal(error.status, 73);
      assert.ok(!error.stdout.includes('systemctl --no-block default'));
      assert.ok(!error.stdout.includes('cloud-config.service'));
      return true;
    },
  );
  const ordinary = JSON.parse(cloudConfig('ssh-ed25519 AAAA test').split('\n').slice(1).join('\n'));
  assert.equal(ordinary.bootcmd, undefined);
});

test('missing boot devices remain failures before any mount or target restart', () => {
  assert.throws(
    () => bootRecovery({ BOOT_MISSING_DEVICE: 'UEFI' }),
    (error) => {
      assert.equal(error.status, 2);
      assert.ok(!error.stdout.includes('systemctl start'));
      return true;
    },
  );
});

test('disposable guests disable only needrestart kernel-image scanning before apt runs', () => {
  const config = JSON.parse(cloudConfig('ssh-ed25519 AAAA test').split('\n').slice(1).join('\n'));
  const file = config.write_files.find(
    (entry) => entry.path === '/etc/needrestart/conf.d/99-mixrelay-qualification.conf',
  );
  assert.equal(file.permissions, '0644');
  const settings = execFileSync(
    'perl',
    [
      '-e',
      'our %nrconf = (kernelhints => 1, restart => "i", interpscan => 1); eval $ARGV[0]; die $@ if $@; print join("\\t", map { "$_=$nrconf{$_}" } sort keys %nrconf);',
      file.content,
    ],
    { encoding: 'utf8' },
  );
  assert.deepEqual(settings.split('\t'), ['interpscan=1', 'kernelhints=0', 'restart=i']);
});
test('QEMU uses the native guest CPU and binds SSH only to the hosted runner loopback', () => {
  const settings = {
    disk: '/tmp/disk',
    seed: '/tmp/seed',
    firmware: '/tmp/code',
    variables: '/tmp/vars',
    port: 22022,
    serial: '/tmp/serial',
  };
  const arm = qemuArguments({ ...settings, cpuArch: 'arm64', accelerator: 'tcg' });
  assert.equal(arm[1], 'virt,accel=tcg');
  assert.equal(arm[3], 'cortex-a72');
  assert.ok(arm.includes('user,id=net0,hostfwd=tcp:127.0.0.1:22022-:22'));
  const network = cloudNetwork().ethernets.qa;
  assert.equal(network.dhcp4, true);
  assert.equal(network.dhcp6, false);
  assert.ok(
    arm.includes(`virtio-net-pci,netdev=net0,mac=${network.match.macaddress},romfile=`),
    'DHCP must match the actual NIC without requiring a PXE ROM',
  );
  const x64 = qemuArguments({ ...settings, cpuArch: 'x64', accelerator: 'kvm' });
  assert.equal(x64[1], 'q35,accel=kvm');
  assert.equal(x64[3], 'host');
  assert.throws(() => qemuArguments({ ...settings, cpuArch: 'arm64', accelerator: 'fake' }));
  const emulated = qemuArguments({ ...settings, cpuArch: 'arm64', accelerator: 'tcg', cpus: guestCpuCount('tcg', 4) });
  assert.equal(emulated[emulated.indexOf('-smp') + 1], '4');
  assert.equal(
    emulated[emulated.indexOf('-smbios') + 1],
    'type=11,value=io.systemd.stub.kernel-cmdline-extra=systemd.default_device_timeout_sec=600',
  );
  assert.equal(x64.includes('-smbios'), false);
  assert.equal(guestCpuCount('kvm', 4), 2);
  assert.equal(guestCpuCount('tcg', 2), 2);
  assert.equal(guestCpuCount('tcg', 1), 1);
  assert.throws(() => guestCpuCount('tcg', 0));
});

test('candidate transfer includes the pinned baseline directory in the same guest', async (t) => {
  const { mkdtempSync, mkdirSync, writeFileSync, rmSync } = await import('node:fs');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const { copyCandidates } = await import('./github-vm.mjs');
  const root = mkdtempSync(join(tmpdir(), 'mixrelay-vm-inputs-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'baseline'));
  writeFileSync(join(root, 'candidate.deb'), 'candidate');
  writeFileSync(join(root, 'baseline/baseline.deb'), 'baseline');
  const calls = [];
  await copyCandidates(async (...args) => calls.push(args), ['-i', '/private/key'], 22222, root);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0][1], [
    '-i',
    '/private/key',
    '-P',
    '22222',
    '-r',
    join(root, 'baseline'),
    join(root, 'candidate.deb'),
    'qa@127.0.0.1:/home/qa/candidate/',
  ]);
  assert.equal(calls[0][2].timeout, 300000);
});
