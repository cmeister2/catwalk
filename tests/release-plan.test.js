import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Writable } from 'node:stream';
import test from 'node:test';
import { pathToFileURL } from 'node:url';
import config from '../release.config.js';
import { createPlan, planningPlugins } from '../.github/release/plan.mjs';
import { createArchive, inspectArchive, payloadFiles, sha256 } from '../.github/release/package.mjs';
import { TARGETS, archiveName, validateCommit, validateVersion } from '../.github/release/targets.mjs';
import { verifyAssets } from '../.github/release/verify.mjs';

const version = '1.0.1';
const commit = 'a'.repeat(40);
const rustVersion = '1.99.0';
const quiet = () => new Writable({ write(_chunk, _encoding, done) { done(); } });

async function temporary(t) {
  const directory = await mkdtemp(join(tmpdir(), 'catwalk-release-test-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}

function git(cwd, ...args) {
  return execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

async function checkout(t, message) {
  const directory = await temporary(t);
  const cwd = join(directory, 'checkout');
  const remote = join(directory, 'remote.git');
  await mkdir(join(cwd, 'src'), { recursive: true });
  await writeFile(join(cwd, 'Cargo.toml'), [
    '# Retain this manifest comment.', '[package]', 'name = "catwalk3d"',
    'version = "0.1.0"', 'edition = "2024"', 'license = "MIT"', '',
  ].join('\n'));
  await writeFile(join(cwd, 'src/main.rs'), 'fn main() {}\n');
  execFileSync('cargo', ['generate-lockfile', '--offline'], { cwd, stdio: 'pipe' });
  git(cwd, 'init', '--initial-branch=main');
  git(cwd, 'config', 'user.name', 'Release tests');
  git(cwd, 'config', 'user.email', 'release-tests@example.invalid');
  git(cwd, 'add', 'Cargo.toml', 'Cargo.lock', 'src/main.rs');
  git(cwd, '-c', 'commit.gpgsign=false', 'commit', '--no-verify', '-m', 'feat: initial crate');
  git(cwd, 'tag', '1.0.0');
  git(directory, 'init', '--bare', remote);
  git(cwd, 'remote', 'add', 'origin', remote);
  git(cwd, '-c', 'commit.gpgsign=false', 'commit', '--no-verify', '--allow-empty', '-m', message);
  git(cwd, 'push', 'origin', 'main', '--tags');
  const env = { ...process.env, CI: 'false' };
  for (const key of Object.keys(env)) {
    if (key.startsWith('GITHUB_') || ['GH_TOKEN', 'GITHUB_TOKEN'].includes(key)) delete env[key];
  }
  return { cwd, remote, env, config: { ...config, repositoryUrl: pathToFileURL(remote).href, noCi: true } };
}

test('planning retains the configured analyzer and pinned notes generator only', () => {
  const plugins = planningPlugins(config);
  assert.equal(plugins.length, 2);
  assert.equal(plugins[0][0], '@semantic-release/commit-analyzer');
  assert.equal(plugins[0][1].preset, 'conventionalcommits');
  assert.equal(plugins[1][1].preset, 'conventionalcommits');
  assert.ok(!JSON.stringify(plugins).includes('semantic-release-cargo'));
  assert.throws(() => planningPlugins({ plugins: [] }), /Missing release planning plugin/);
});

test('CI preparation leaves source versions unchanged and records exact provenance', async (t) => {
  const options = await checkout(t, 'fix: improve thumbnails');
  const before = await readFile(join(options.cwd, 'Cargo.toml'));
  const lock = await readFile(join(options.cwd, 'Cargo.lock'));
  const output = join(options.cwd, 'outputs.txt');
  const plan = await createPlan({ ...options, output, stdout: quiet(), stderr: quiet() });
  assert.equal(plan.schema, 1);
  assert.equal(plan.version, '0.1.0');
  assert.equal(plan.releasing, false);
  assert.equal(plan.commit, git(options.cwd, 'rev-parse', 'HEAD'));
  assert.equal(plan.rustVersion, execFileSync('rustc', ['--version'], { encoding: 'utf8' }).split(/\s+/)[1]);
  assert.deepEqual(await readFile(join(options.cwd, 'Cargo.toml')), before);
  assert.deepEqual(await readFile(join(options.cwd, 'Cargo.lock')), lock);
  assert.deepEqual(await readFile(join(options.cwd, 'release-prepared/Cargo.toml')), before);
  assert.deepEqual(await readFile(join(options.cwd, 'release-prepared/Cargo.lock')), lock);
  assert.deepEqual(JSON.parse(await readFile(join(options.cwd, 'release-prepared/plan.json'))), plan);
  const outputs = Object.fromEntries((await readFile(output, 'utf8')).trim().split('\n').map(
    (line) => [line.slice(0, line.indexOf('=')), line.slice(line.indexOf('=') + 1)]
  ));
  assert.deepEqual(outputs, {
    version: '0.1.0', releasing: 'false', commit: plan.commit, 'rust-version': plan.rustVersion,
    matrix: JSON.stringify({ include: TARGETS }),
  });
});

test('real release planning and Cargo preparation stamp the manifest and lockfile before compilation', {
  skip: process.platform === 'win32' && 'The standard Cargo plugin has no Windows NAPI binding.',
}, async (t) => {
  const options = await checkout(t, 'fix: improve thumbnails');
  const before = await readFile(join(options.cwd, 'Cargo.toml'), 'utf8');
  const lock = await readFile(join(options.cwd, 'Cargo.lock'), 'utf8');
  const tags = git(options.cwd, 'tag');
  const plan = await createPlan({ ...options, release: true, stdout: quiet(), stderr: quiet() });
  assert.equal(plan.version, version);
  assert.equal(plan.releasing, true);
  assert.equal(await readFile(join(options.cwd, 'Cargo.toml'), 'utf8'), before.replace('version = "0.1.0"', `version = "${version}"`));
  assert.equal(await readFile(join(options.cwd, 'Cargo.lock'), 'utf8'), lock.replace('version = "0.1.0"', `version = "${version}"`));
  assert.equal(git(options.cwd, 'tag'), tags, 'Dry-run planning must not create a release tag.');
  assert.equal(git(options.cwd, '--git-dir', options.remote, 'rev-parse', 'main'), plan.commit);
  execFileSync('cargo', ['check', '--offline', '--locked'], { cwd: options.cwd, stdio: 'pipe' });
});

test('a documentation-only main commit produces a nonpublishing plan', async (t) => {
  const options = await checkout(t, 'docs: explain thumbnail options');
  const before = await readFile(join(options.cwd, 'Cargo.toml'));
  const plan = await createPlan({ ...options, release: true, stdout: quiet(), stderr: quiet() });
  assert.equal(plan.releasing, false);
  assert.equal(plan.version, '0.1.0');
  assert.deepEqual(await readFile(join(options.cwd, 'Cargo.toml')), before);
});

/** Create independent complete target payloads for publication-integrity controls. */
async function assets(t) {
  const directory = await temporary(t);
  for (const definition of TARGETS) {
    const release = { schema: 1, version, commit, rustVersion, target: definition.target, binary: definition.binary };
    const files = new Map(payloadFiles(definition.target).map((name) => [name, Buffer.from(
      name === 'release.json' ? JSON.stringify(release) : name === definition.binary ? `Executable for ${definition.target}` : `Shared ${name}`
    )]));
    await saveAsset(directory, definition.target, files, release);
  }
  return directory;
}

async function saveAsset(directory, target, files, release) {
  const archive = archiveName(version, target);
  const bytes = createArchive({ version, target, files });
  const members = inspectArchive(bytes, { version, target });
  const metadata = {
    ...release, archive, sha256: sha256(bytes),
    files: [...members].map(([path, contents]) => ({ path, size: contents.length, sha256: sha256(contents) })),
  };
  await writeFile(join(directory, archive), bytes);
  await writeFile(join(directory, archive.replace(/\.zip$/, '.json')), JSON.stringify(metadata));
  await writeFile(join(directory, `${archive}.sha256`), `${metadata.sha256}  ${archive}\n`);
}

test('the pre-tag verifier admits all seven targets and writes sorted download checksums', async (t) => {
  const directory = await assets(t);
  const archives = await verifyAssets(version, commit, directory);
  assert.equal(archives.length, 7);
  const lines = (await readFile(join(directory, 'SHA256SUMS'), 'utf8')).trim().split('\n');
  const filenames = lines.map((line) => line.split('  ')[1]);
  assert.deepEqual(filenames, [...filenames].sort((left, right) => left.localeCompare(right, 'en')));
  for (const archive of archives) {
    assert.ok(lines.includes(`${sha256(await readFile(join(directory, archive)))}  ${archive}`));
  }
});

for (const [label, mutate, expected] of [
  ['a missing platform', async (directory, name) => rm(join(directory, name)), /exactly every configured target/],
  ['an unexpected asset', async (directory) => writeFile(join(directory, 'unexpected.zip'), 'extra'), /exactly every configured target/],
  ['tampered archive bytes', async (directory, name) => writeFile(join(directory, name), 'modified'), /checksum does not match/],
  ['a forged checksum file', async (directory, name) => writeFile(join(directory, `${name}.sha256`), `${'0'.repeat(64)}  ${name}\n`), /checksum file is invalid/],
  ['a stale commit', async (directory, name) => {
    const path = join(directory, name.replace(/\.zip$/, '.json'));
    const metadata = JSON.parse(await readFile(path));
    metadata.commit = 'b'.repeat(40);
    await writeFile(path, JSON.stringify(metadata));
  }, /stale commit/],
  ['a mismatched compiler', async (directory, name) => {
    const path = join(directory, name.replace(/\.zip$/, '.json'));
    const metadata = JSON.parse(await readFile(path));
    metadata.rustVersion = '1.98.0';
    await writeFile(path, JSON.stringify(metadata));
  }, /stale release metadata|same Rust compiler/],
  ['an incorrect member digest', async (directory, name) => {
    const path = join(directory, name.replace(/\.zip$/, '.json'));
    const metadata = JSON.parse(await readFile(path));
    metadata.files[0].sha256 = '0'.repeat(64);
    await writeFile(path, JSON.stringify(metadata));
  }, /checksum does not match/],
]) {
  test(`the pre-tag verifier rejects ${label}`, async (t) => {
    const directory = await assets(t);
    await mutate(directory, archiveName(version, TARGETS[0].target));
    await assert.rejects(verifyAssets(version, commit, directory), expected);
  });
}

test('stale metadata inside a fully rehashed archive is rejected', async (t) => {
  const directory = await assets(t);
  const definition = TARGETS[0];
  const archive = archiveName(version, definition.target);
  const metadata = JSON.parse(await readFile(join(directory, archive.replace(/\.zip$/, '.json'))));
  const members = inspectArchive(await readFile(join(directory, archive)), { version, target: definition.target });
  const files = new Map([...members].map(([path, bytes]) => [path.slice(path.lastIndexOf('/') + 1), bytes]));
  const release = { schema: 1, version, commit, rustVersion, target: definition.target, binary: definition.binary };
  files.set('release.json', Buffer.from(JSON.stringify({ ...release, version: '1.0.0' })));
  await saveAsset(directory, definition.target, files, release);
  assert.equal(metadata.version, version);
  await assert.rejects(verifyAssets(version, commit, directory), /stale release metadata/);
});

test('release identifiers reject path characters and noncanonical versions', () => {
  for (const value of ['../1.0.0', '01.0.0', '1.0', '1.0.0-01', '1.0.0\n']) {
    assert.throws(() => validateVersion(value), /Invalid release version/);
  }
  assert.equal(validateVersion('1.2.3-beta.1+build.01'), '1.2.3-beta.1+build.01');
  assert.throws(() => validateCommit('../commit'), /full lowercase Git commit/);
  assert.throws(() => archiveName(version, '../target'), /Unsupported release target/);
});
