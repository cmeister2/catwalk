import assert from 'node:assert/strict';
import { mkdir, mkdtemp, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { assertSeparateDirectories, verifyWebOutput } from '../.github/web/output.mjs';

async function page(t) {
  const directory = await mkdtemp(join(tmpdir(), 'catwalk-page-output-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await mkdir(join(directory, 'vendor'));
  for (const name of [
    'index.html', 'app.js', 'config.js', 'style.css', 'worker.js', 'wasi-runner.js',
    'vendor/wasi.js', 'catwalk.wasm', 'catwalk.wasm.gz', 'licenses.txt', 'build.json',
  ]) await writeFile(join(directory, name), 'runtime asset');
  return directory;
}

test('page and Cargo directories are disjoint, including profile host output', () => {
  assertSeparateDirectories('/tmp/project/target/web', '/tmp/project/target/web-build');
  for (const [output, target] of [
    ['/tmp/project/target/web', '/tmp/project/target'],
    ['/tmp/project/target', '/tmp/project/target/web-build'],
    ['/tmp/project/target/web', '/tmp/project/target/web'],
  ]) assert.throws(() => assertSeparateDirectories(output, target), /must be separate/);
});

test('only static runtime assets may be published, with full notices when requested', async t => {
  const directory = await page(t);
  assert.equal((await verifyWebOutput(directory)).length, 11);
  await assert.rejects(verifyWebOutput(directory, { requireNotices: true }), /Missing.*LICENSES/);
  for (const name of ['THIRD_PARTY_LICENSES.html', 'RUST-LICENSES.html', 'MUSL-COPYRIGHT', 'WASI-LIBC-LICENSES.txt']) {
    await writeFile(join(directory, name), 'original notice');
  }
  assert.equal((await verifyWebOutput(directory, { requireNotices: true })).length, 15);
});

test('Cargo host artifacts in the page directory prevent publication', async t => {
  const directory = await page(t);
  await mkdir(join(directory, 'deps'));
  await writeFile(join(directory, 'deps', 'libproc_macro.so'), 'host compiler artifact');
  await assert.rejects(verifyWebOutput(directory), /Unexpected.*deps/);
});

test('missing wasm, stale files and symlinks prevent publication', async t => {
  const directory = await page(t);
  await rm(join(directory, 'catwalk.wasm'));
  await assert.rejects(verifyWebOutput(directory), /Missing.*catwalk\.wasm/);
  await writeFile(join(directory, 'catwalk.wasm'), 'module');
  await writeFile(join(directory, 'stale.txt'), 'unrelated');
  await assert.rejects(verifyWebOutput(directory), /Unexpected.*stale\.txt/);
  await rm(join(directory, 'stale.txt'));
  await symlink('index.html', join(directory, 'linked.html'));
  await assert.rejects(verifyWebOutput(directory), /Unexpected.*linked\.html/);
});
