import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { zipSync } from 'fflate';
import {
  createArchive, inspectArchive, MAX_PAYLOAD_BYTES, packageBinary,
  payloadFiles, payloadRoot, sha256, validateNativeHost,
} from '../.github/release/package.mjs';
import { TARGETS } from '../.github/release/targets.mjs';

const VERSION = '1.2.3';
const TARGET = 'x86_64-unknown-linux-gnu';

/** Enable the independent Unix permission check when the system extractor is available. */
function hasUnixUnzip() {
  if (process.platform === 'win32') return false;
  try {
    execFileSync('unzip', ['-v'], { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

/** Create distinguishable members so a successful round trip verifies every byte. */
function fixtureFiles(target = TARGET) {
  return new Map(payloadFiles(target).map((name) => [name, Buffer.from(`Contents of ${target}/${name}\n`)]));
}

/** Use the standard writer to construct deliberately incomplete or unexpected payloads. */
function rawArchive(files, { version = VERSION, target = TARGET, level = 0 } = {}) {
  return zipSync(Object.fromEntries([...files].map(([name, bytes]) => [
    `${payloadRoot(version, target)}/${name}`, bytes,
  ])), { level });
}

/** Replace equal-length filenames in both ZIP records to exercise duplicate rejection. */
function replaceFilename(bytes, before, after) {
  const source = Buffer.from(before);
  const replacement = Buffer.from(after);
  assert.equal(source.length, replacement.length);
  const mutated = Buffer.from(bytes);
  let position = mutated.indexOf(source);
  let occurrences = 0;
  while (position !== -1) {
    replacement.copy(mutated, position);
    occurrences += 1;
    position = mutated.indexOf(source, position + source.length);
  }
  assert.equal(occurrences, 2);
  return mutated;
}

/** Alter central-directory test metadata without allocating the advertised payload. */
function alterCentralField(bytes, offset, value, width = 4, count = 1) {
  const mutated = Buffer.from(bytes);
  const signature = Buffer.from([80, 75, 1, 2]);
  let position = mutated.indexOf(signature);
  for (let entry = 0; entry < count; entry += 1) {
    assert.notEqual(position, -1);
    if (width === 2) mutated.writeUInt16LE(value, position + offset);
    else mutated.writeUInt32LE(value, position + offset);
    position = mutated.indexOf(signature, position + signature.length);
  }
  return mutated;
}

for (const { target, binary, musl } of TARGETS) {
  test(`release ZIP round trip preserves the ${target} payload`, () => {
    const files = fixtureFiles(target);
    const archive = createArchive({ version: VERSION, target, files });
    const members = inspectArchive(archive, { version: VERSION, target });
    assert.equal(members.size, musl ? 7 : 6);
    assert.ok(members.has(`${payloadRoot(VERSION, target)}/${binary}`));
    assert.equal(members.has(`${payloadRoot(VERSION, target)}/MUSL-COPYRIGHT`), musl);
    for (const [name, bytes] of files) {
      assert.deepEqual(Buffer.from(members.get(`${payloadRoot(VERSION, target)}/${name}`)), bytes);
    }
    assert.match(sha256(archive), /^[\da-f]{64}$/);
  });
}

test('the ZIP executable mode survives extraction with the system unzip', {
  skip: !hasUnixUnzip(),
}, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'catwalk-zip-mode-'));
  try {
    const files = fixtureFiles();
    const archive = join(directory, 'release.zip');
    await writeFile(archive, createArchive({ version: VERSION, target: TARGET, files }));
    const extracted = join(directory, 'extracted');
    execFileSync('unzip', ['-q', archive, '-d', extracted], { stdio: 'ignore', timeout: 10_000 });
    const root = join(extracted, payloadRoot(VERSION, TARGET));
    assert.equal((await stat(join(root, 'catwalk'))).mode & 0o777, 0o755);
    assert.equal((await stat(join(root, 'LICENSE'))).mode & 0o777, 0o644);
    assert.deepEqual(await readFile(join(root, 'catwalk')), files.get('catwalk'));
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('the packaging allowlist rejects missing, extra, empty and non-byte files', () => {
  const missing = fixtureFiles();
  missing.delete('LICENSE');
  assert.throws(() => createArchive({ version: VERSION, target: TARGET, files: missing }), /exact expected files/);
  const extra = fixtureFiles();
  extra.set('.env', Buffer.from('unwanted file'));
  assert.throws(() => createArchive({ version: VERSION, target: TARGET, files: extra }), /exact expected files/);
  for (const invalid of [new Uint8Array(), 'text']) {
    const files = fixtureFiles();
    files.set('catwalk', invalid);
    assert.throws(() => createArchive({ version: VERSION, target: TARGET, files }), /Invalid or oversized/);
  }
});

test('archive inspection rejects missing and unexpected members before extraction', () => {
  const missing = fixtureFiles();
  missing.delete('LICENSE');
  assert.throws(() => inspectArchive(rawArchive(missing), { version: VERSION, target: TARGET }), /missing/);
  for (const name of ['../catwalk', '/catwalk', 'folder/catwalk', '.env', 'MUSL-COPYRIGHT']) {
    const files = fixtureFiles();
    files.set(name, Buffer.from('unexpected'));
    assert.throws(() => inspectArchive(rawArchive(files), { version: VERSION, target: TARGET }), /Unexpected ZIP member/);
  }
});

test('archive inspection detects duplicate entries before dictionary coalescing', () => {
  const archive = rawArchive(fixtureFiles());
  const root = payloadRoot(VERSION, TARGET);
  const duplicate = replaceFilename(archive, `${root}/LICENSE`, `${root}/catwalk`);
  assert.throws(() => inspectArchive(duplicate, { version: VERSION, target: TARGET }), /Duplicate ZIP member/);
});

test('archive inspection rejects unsupported compression before codec dispatch', () => {
  const archive = alterCentralField(rawArchive(fixtureFiles()), 10, 14, 2);
  assert.throws(() => inspectArchive(archive, { version: VERSION, target: TARGET }), /Unsupported ZIP compression/);
});

test('archive inspection rejects oversized and zero-length advertised members', () => {
  const archive = rawArchive(fixtureFiles());
  for (const originalSize of [MAX_PAYLOAD_BYTES + 1, 0]) {
    const invalid = alterCentralField(archive, 24, originalSize);
    assert.throws(() => inspectArchive(invalid, { version: VERSION, target: TARGET }), /Invalid or oversized ZIP member/);
  }
});

test('archive inspection bounds combined advertised sizes without expanding them', () => {
  // Stored members remain small; only their declared original sizes are increased.
  const archive = alterCentralField(rawArchive(fixtureFiles()), 24, 64 * 1024 * 1024, 4, 5);
  assert.throws(() => inspectArchive(archive, { version: VERSION, target: TARGET }), /Combined ZIP payload is oversized/);
});

test('archive inspection rejects decoded sizes that disagree with central metadata', () => {
  const files = fixtureFiles();
  const archive = alterCentralField(rawArchive(files), 24, files.get('catwalk').length + 1);
  assert.throws(() => inspectArchive(archive, { version: VERSION, target: TARGET }), /size does not match/);
});

test('archive inspection rejects truncated archives and different target or version roots', () => {
  const archive = createArchive({ version: VERSION, target: TARGET, files: fixtureFiles() });
  assert.throws(() => inspectArchive(archive.subarray(0, archive.length - 50), { version: VERSION, target: TARGET }));
  assert.throws(() => inspectArchive(archive, { version: '1.2.4', target: TARGET }), /Unexpected ZIP member/);
  assert.throws(() => inspectArchive(archive, { version: VERSION, target: 'aarch64-unknown-linux-gnu' }), /Unexpected ZIP member/);
});

test('native target checks reject architecture and operating system mismatches', () => {
  for (const { target } of TARGETS) {
    const platform = target.includes('-linux-') ? 'linux' : target.includes('-apple-') ? 'darwin' : 'win32';
    const architecture = target.startsWith('aarch64-') ? 'arm64' : 'x64';
    assert.doesNotThrow(() => validateNativeHost(target, platform, architecture));
    assert.throws(() => validateNativeHost(target, 'unsupported', architecture), /does not match native host/);
    assert.throws(() => validateNativeHost(target, platform, architecture === 'x64' ? 'arm64' : 'x64'), /does not match native host/);
  }
});

test('invalid prepared metadata stops packaging before invoking Cargo or creating assets', async () => {
  const target = TARGETS.find(({ target: value }) => {
    try { validateNativeHost(value); return true; } catch { return false; }
  })?.target;
  assert.ok(target, 'the test host has a supported release target');
  const cwd = await mkdtemp(join(tmpdir(), 'catwalk-release-test-'));
  try {
    await mkdir(join(cwd, 'release-prepared'));
    await writeFile(join(cwd, 'release-prepared/plan.json'), JSON.stringify({
      schema: 1, version: VERSION, commit: '../invalid', rustVersion: '1.99.0', releasing: false,
    }));
    await assert.rejects(packageBinary(target, { cwd }), /full lowercase Git commit/);
  } finally {
    await rm(cwd, { recursive: true, force: true });
  }
});
