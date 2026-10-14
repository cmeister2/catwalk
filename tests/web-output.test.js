import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
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

test('primary file picker accepts exactly the advertised Rust extensions', async () => {
  const html = await readFile(new URL('../web/index.html', import.meta.url), 'utf8');
  const picker = html.match(/<input\b[^>]*\bid="primary-file"[^>]*>/);
  assert.ok(picker, 'primary file picker exists');
  const accept = picker[0].match(/\baccept="([^"]+)"/);
  assert.ok(accept, 'primary file picker has a nonempty accept filter');
  const actual = accept[1].split(',').map(value => value.trim());
  assert.ok(actual.every(value => /^\.[a-z0-9]+$/.test(value)), 'use explicit file extensions, not broad MIME wildcards');
  assert.equal(new Set(actual).size, actual.length, 'accept entries are unique');

  const expected = new Set();
  for (const [path, names] of [
    ['../src/formats/mod.rs', ['MODEL_EXTENSIONS', 'IMAGE_EXTENSIONS', 'NATIVE_GEOMETRY_EXTENSIONS']],
    ['../src/saved_preview.rs', ['EXTENSIONS']],
  ]) {
    const source = await readFile(new URL(path, import.meta.url), 'utf8');
    for (const name of names) {
      const registry = source.match(new RegExp(`pub const ${name}:\\s*&\\[&str\\]\\s*=\\s*&\\[([\\s\\S]*?)\\];`));
      assert.ok(registry, `Rust extension registry ${name} exists`);
      const extensions = [...registry[1].matchAll(/"([a-z0-9]+)"/g)];
      assert.ok(extensions.length > 0, `${name} contains extensions`);
      for (const [, extension] of extensions) expected.add(`.${extension}`);
    }
  }
  assert.deepEqual(actual.sort(), [...expected].sort());
});

test('companion file picker remains unrestricted for arbitrary resource extensions', async () => {
  const html = await readFile(new URL('../web/index.html', import.meta.url), 'utf8');
  const picker = html.match(/<input\b[^>]*\bid="companion-files"[^>]*>/);
  assert.ok(picker, 'companion file picker exists');
  assert.doesNotMatch(picker[0], /\baccept\s*=/);
});
