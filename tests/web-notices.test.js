import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import {
  bundledWasiSdk, generateWasiLibcNotices, renderOriginalWasiNotice,
  renderWasiLibcNotices, wasiLibcSources,
} from '../.github/web/notices.mjs';

const commit = 'b940084d7eb6a299eb4bfeb8e34901bc051e7ac4';
const libc = '161b3195fc2558d2b1ba3eb9ffae3b2b47407623';
const verboseVersion = 'release: 1.99.0\ncommit-hash: ' + commit + '\n';
const dockerUrl = 'https://raw.githubusercontent.com/rust-lang/rust/' + commit + '/src/ci/docker/host-x86_64/dist-various-2/Dockerfile';
const docker = (version = '33.0', tag = version.split('.')[0]) =>
  'ENV TARGETS=$TARGETS,wasm32-wasip1\nRUN curl -L https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-' + tag + '/wasi-sdk-' + version + '-x86_64-linux.tar.gz | tar -xz\n';
const hash = bytes => createHash('sha256').update(bytes).digest('hex');

async function directory(t, rustVersion = '1.99.0') {
  const path = await mkdtemp(join(tmpdir(), 'catwalk-web-notices-'));
  t.after(() => rm(path, { recursive: true, force: true }));
  await writeFile(join(path, 'plan.json'), JSON.stringify({ schema: 1, rustVersion }));
  await writeFile(join(path, 'MUSL-COPYRIGHT'), 'placeholder for failure controls');
  return path;
}

test('WASI libc notices follow the pinned Rust SDK download and reject new or ambiguous SDKs', () => {
  assert.equal(bundledWasiSdk(docker()), '33.0');
  assert.equal(bundledWasiSdk(docker() + docker()), '33.0');
  assert.throws(() => bundledWasiSdk(docker('34.0')), /requires qualified/);
  assert.throws(() => bundledWasiSdk(docker('33.0', '32')), /disagree/);
  assert.throws(() => bundledWasiSdk(docker() + docker('34.0')), /unambiguous/);
  assert.throws(() => bundledWasiSdk('ENV TARGETS=wasm32-wasip1\n'), /unambiguous/);
  assert.throws(() => bundledWasiSdk(docker().replace('wasm32-wasip1', 'wasm32-wasip2')), /does not identify/);
});

test('qualified runtime sources use immutable libc paths with verified checksums', () => {
  const sources = wasiLibcSources('33.0');
  assert.deepEqual(sources.map(source => source.path), [
    'LICENSE', 'LICENSE-MIT', 'libc-bottom-half/cloudlibc/LICENSE',
    'fts/musl-fts/COPYING', 'dlmalloc/src/malloc.c',
  ]);
  for (const source of sources) {
    assert.equal(source.url, 'https://raw.githubusercontent.com/WebAssembly/wasi-libc/' + libc + '/' + source.path);
    assert.match(source.sha256, /^[0-9a-f]{64}$/);
  }
  assert.equal(sources[2].sha256, 'c8b789cf5a746611e6300a0cc7750dbf92b61912a709d04e639245f7290656d0');
  assert.equal(sources[4].noticeBytes, 269);
  assert.throws(() => wasiLibcSources('34.0'), /requires qualified/);
});

test('original notice rendering preserves UTF-8 and line endings and refuses substitutions', () => {
  const bytes = Buffer.from('Copyright © example\r\nWhole terms: <tag> & "quotes"\r\n');
  const source = { path: 'LICENSE', url: 'https://example.invalid/pinned/LICENSE', sha256: hash(bytes) };
  const rendered = renderOriginalWasiNotice(source, bytes);
  assert(rendered.includes(bytes));
  assert.throws(() => renderOriginalWasiNotice(source, Buffer.from('changed')), /checksum differs/);
  const invalid = Buffer.from([0xff]);
  assert.throws(() => renderOriginalWasiNotice({ ...source, sha256: hash(invalid) }, invalid), /not UTF-8/);
  assert.throws(() => renderOriginalWasiNotice(source, Buffer.alloc(1024 * 1024 + 1)), /size limit/);
});

test('allocator packaging retains only its independently checked original notice prefix', () => {
  const prefix = Buffer.from('Original CC0 notice\n');
  const bytes = Buffer.concat([prefix, Buffer.from('allocator implementation is not distributed')]);
  const source = { path: 'malloc.c', url: 'https://example.invalid/pinned/malloc.c', sha256: hash(bytes),
    noticeBytes: prefix.length, noticeSha256: hash(prefix) };
  const output = renderOriginalWasiNotice(source, bytes);
  assert(output.includes(prefix));
  assert(!output.includes(Buffer.from('allocator implementation')));
  assert.throws(() => renderOriginalWasiNotice({ ...source, noticeSha256: '0'.repeat(64) }, bytes), /prefix checksum/);
});

test('complete runtime bundle requires every original notice and matching existing musl bytes', () => {
  assert.throws(() => renderWasiLibcNotices('33.0', [], Buffer.alloc(0)), /every qualified/);
  const contents = wasiLibcSources('33.0').map(() => Buffer.alloc(0));
  assert.throws(() => renderWasiLibcNotices('33.0', contents, Buffer.from('different musl')), /MUSL-COPYRIGHT differs/);
});

test('generator binds the prepared version and actual compiler before network or output', async t => {
  const prepared = await directory(t, '1.98.0');
  let called = false;
  await assert.rejects(generateWasiLibcNotices(prepared, prepared, {
    verboseVersion, load: async () => { called = true; return Buffer.alloc(0); },
  }), /differs/);
  assert.equal(called, false);
  await assert.rejects(readFile(join(prepared, 'WASI-LIBC-LICENSES.txt')), /ENOENT/);
});

test('generator retrieves compiler-commit configuration and fails closed on unqualified SDK', async t => {
  const prepared = await directory(t);
  const calls = [];
  await assert.rejects(generateWasiLibcNotices(prepared, prepared, {
    verboseVersion, load: async url => { calls.push(url); return Buffer.from(docker('34.0')); },
  }), /requires qualified/);
  assert.deepEqual(calls, [dockerUrl]);
  await assert.rejects(readFile(join(prepared, 'WASI-LIBC-LICENSES.txt')), /ENOENT/);
});

test('generator rejects oversized builder metadata and substituted upstream notices without overwriting output', async t => {
  const prepared = await directory(t);
  const output = join(prepared, 'WASI-LIBC-LICENSES.txt');
  await writeFile(output, 'retain previously verified output');
  await assert.rejects(generateWasiLibcNotices(prepared, prepared, {
    verboseVersion, load: async () => Buffer.alloc(1024 * 1024 + 1),
  }), /size limit/);
  const calls = [];
  await assert.rejects(generateWasiLibcNotices(prepared, prepared, {
    verboseVersion, load: async url => {
      calls.push(url);
      return Buffer.from(url === dockerUrl ? docker() : 'substituted notice');
    },
  }), /notice checksum differs/);
  assert.equal(calls[0], dockerUrl);
  assert.equal(calls.length, 1 + wasiLibcSources('33.0').length);
  assert.equal(await readFile(output, 'utf8'), 'retain previously verified output');
});
