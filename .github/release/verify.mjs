import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { lstat, readFile, readdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { inspectArchive, payloadRoot } from './package.mjs';
import { TARGETS, archiveName, validateCommit, validateVersion } from './targets.mjs';

const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

/** Read a regular asset within its expected size bound. */
async function assetBytes(directory, name, limit) {
  const path = join(directory, name);
  const info = await lstat(path);
  if (!info.isFile() || info.size === 0 || info.size > limit) {
    throw new Error(`Invalid release asset: ${name}`);
  }
  const bytes = await readFile(path);
  if (bytes.length !== info.size) throw new Error(`Release asset changed while reading: ${name}`);
  return bytes;
}

/** Reject incomplete, unexpected or misspelled metadata fields. */
function requireKeys(value, keys, label) {
  assert.ok(value && typeof value === 'object' && !Array.isArray(value), `${label} must be an object.`);
  assert.deepEqual(Object.keys(value).sort(), [...keys].sort(), `${label} has unexpected fields.`);
}

/** Check every target before semantic-release creates a tag or publishes the crate. */
export async function verifyAssets(version, commit, directory = resolve('release-assets')) {
  validateVersion(version);
  validateCommit(commit);
  const names = TARGETS.map(({ target }) => archiveName(version, target));
  const expectedNames = names.flatMap((name) => [name, name.replace(/\.zip$/, '.json'), `${name}.sha256`]);
  const present = (await readdir(directory)).filter((name) => name !== 'SHA256SUMS').sort();
  assert.deepEqual(present, expectedNames.sort(), 'Release assets must contain exactly every configured target.');
  const checksums = [];
  let compiler;
  const sharedFiles = new Map();
  for (const definition of TARGETS) {
    const archive = archiveName(version, definition.target);
    const bytes = await assetBytes(directory, archive, 256 * 1024 * 1024);
    const metadata = JSON.parse(await assetBytes(directory, archive.replace(/\.zip$/, '.json'), 64 * 1024));
    requireKeys(metadata, [
      'schema', 'version', 'commit', 'rustVersion', 'target', 'binary', 'archive', 'sha256', 'files',
    ], archive);
    const release = {
      schema: 1, version, commit, rustVersion: validateVersion(metadata.rustVersion),
      target: definition.target, binary: definition.binary,
    };
    for (const [key, value] of Object.entries(release)) {
      assert.equal(metadata[key], value, `${archive} has stale ${key}.`);
    }
    compiler ??= release.rustVersion;
    assert.equal(release.rustVersion, compiler, 'All release targets must use the same Rust compiler.');
    assert.equal(metadata.archive, archive, 'Archive metadata has an unexpected filename.');
    const checksum = digest(bytes);
    assert.equal(metadata.sha256, checksum, `${archive} checksum does not match its metadata.`);
    const checksumFile = await assetBytes(directory, `${archive}.sha256`, 2048);
    assert.equal(checksumFile.toString('utf8'), `${checksum}  ${archive}\n`, `${archive} checksum file is invalid.`);
    const members = inspectArchive(bytes, { version, target: definition.target });
    assert.ok(Array.isArray(metadata.files), `${archive} member metadata must be an array.`);
    assert.equal(metadata.files.length, members.size, `${archive} member metadata is incomplete.`);
    const seen = new Set();
    for (const file of metadata.files) {
      requireKeys(file, ['path', 'sha256', 'size'], `${archive} member`);
      assert.ok(!seen.has(file.path), `${archive} repeats member metadata.`);
      seen.add(file.path);
      const member = members.get(file.path);
      assert.ok(member, `${archive} declares an unexpected member: ${file.path}`);
      assert.equal(file.size, member.length, `${file.path} has an incorrect size.`);
      assert.equal(file.sha256, digest(member), `${file.path} checksum does not match.`);
      const filename = file.path.slice(payloadRoot(version, definition.target).length + 1);
      if (!['catwalk', 'catwalk.exe', 'release.json'].includes(filename)) {
        if (sharedFiles.has(filename)) {
          assert.equal(file.sha256, sharedFiles.get(filename), `Release targets disagree on ${filename}.`);
        } else {
          sharedFiles.set(filename, file.sha256);
        }
      }
    }
    const contained = JSON.parse(Buffer.from(members.get(`${payloadRoot(version, definition.target)}/release.json`)));
    requireKeys(contained, Object.keys(release), `${archive} release.json`);
    assert.deepEqual(contained, release, `${archive} contains stale release metadata.`);
    checksums.push({ archive, checksum });
  }
  checksums.sort((left, right) => left.archive.localeCompare(right.archive, 'en'));
  await writeFile(join(directory, 'SHA256SUMS'), `${checksums.map(({ archive, checksum }) => `${checksum}  ${archive}`).join('\n')}\n`);
  return names;
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1] ?? '')).href) {
  const args = process.argv.slice(2);
  if (args.length !== 2) {
    console.error('Usage: node .github/release/verify.mjs VERSION COMMIT');
    process.exitCode = 1;
  } else {
    try {
      await verifyAssets(...args);
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
}
