import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import {
  bundledMuslVersion, compilerIdentity, fetchMuslCopyright, fetchVerifiedNotice,
  outputDirectory, sourceBytes, verifyCargoNotices, verifyMuslCopyright,
} from '../.github/release/notices.mjs';

const rustc = `rustc 1.99.0\nrelease: 1.99.0\ncommit-hash: ${'a'.repeat(40)}\n`;

test('notice toolchain must match the prepared release compiler', () => {
  assert.deepEqual(compilerIdentity({ schema: 1, rustVersion: '1.99.0' }, rustc), {
    version: '1.99.0', commit: 'a'.repeat(40),
  });
  assert.throws(() => compilerIdentity({ schema: 1, rustVersion: '1.98.0' }, rustc), /differs/);
  assert.throws(() => compilerIdentity({ schema: 1, rustVersion: '1.99.0' }, 'rustc 1.99.0'), /incomplete/);
});

test('musl notice follows the pinned Rust build, never a host package', () => {
  assert.equal(bundledMuslVersion('make MUSL_VER=1.2.5\nmake install MUSL_VER=1.2.5'), '1.2.5');
  assert.throws(() => bundledMuslVersion('make MUSL_VER=1.2.5\nmake MUSL_VER=1.2.7'), /unambiguous/);
  assert.throws(() => bundledMuslVersion('make MUSL_VER=1.2.7'), /verified/);
  assert.throws(() => bundledMuslVersion('# musl 1.2.5'), /unambiguous/);
  assert.throws(() => verifyMuslCopyright('1.2.5', Buffer.from('substituted notice')), /checksum/);
});

test('an unavailable primary notice uses only checksum-identical fallback bytes', async () => {
  const bytes = Buffer.from('Complete original notice\n');
  const checksum = createHash('sha256').update(bytes).digest('hex');
  const sources = ['https://primary.invalid/COPYRIGHT', 'https://mirror.invalid/pinned/COPYRIGHT'];
  const calls = [];
  const loaded = await fetchVerifiedNotice(sources, checksum, async (source) => {
    calls.push(source);
    if (source === sources[0]) throw new DOMException('Primary timeout', 'TimeoutError');
    return bytes;
  });
  assert.ok(loaded.equals(bytes));
  assert.deepEqual(calls, sources);
  const primaryCalls = [];
  assert.ok((await fetchVerifiedNotice(sources, checksum, async (source) => {
    primaryCalls.push(source);
    return bytes;
  })).equals(bytes));
  assert.deepEqual(primaryCalls, sources.slice(0, 1));
});

test('substituted notices and unavailable sources cannot make a release pass', async () => {
  const bytes = Buffer.from('Complete original notice\n');
  const checksum = createHash('sha256').update(bytes).digest('hex');
  const sources = ['https://primary.invalid/COPYRIGHT', 'https://mirror.invalid/pinned/COPYRIGHT'];
  let calls = 0;
  await assert.rejects(fetchVerifiedNotice(sources, checksum, async () => {
    calls++;
    return bytes.subarray(0, -1);
  }), /checksum/);
  assert.equal(calls, 1, 'A returned changed primary notice must not be hidden by a fallback.');
  await assert.rejects(fetchVerifiedNotice(sources, checksum, async (source) => {
    if (source === sources[0]) throw new Error('Unavailable primary');
    return Buffer.from('Wrong mirror bytes');
  }), /checksum/);
  await assert.rejects(fetchVerifiedNotice(sources, checksum, async () => {
    throw new Error('Unavailable source');
  }), (error) => error instanceof AggregateError && error.errors.length === 2
    && /unavailable/.test(error.message));
  await assert.rejects(fetchVerifiedNotice([], checksum), /one or two/);
  await assert.rejects(fetchMuslCopyright('1.2.7'), /verified upstream/);
  const muslCalls = [];
  await assert.rejects(fetchMuslCopyright('1.2.5', async (source) => {
    muslCalls.push(source);
    if (muslCalls.length === 1) throw new DOMException('Primary timeout', 'TimeoutError');
    return Buffer.from('Wrong musl copyright');
  }), /checksum/);
  assert.deepEqual(muslCalls, [
    'https://git.musl-libc.org/cgit/musl/plain/COPYRIGHT?h=v1.2.5',
    'https://raw.githubusercontent.com/ifduyue/musl/0784374d561435f7c787a555aeab8ede699ed298/COPYRIGHT',
  ]);
});

test('every notice source gets a timeout signal and a streaming 1 MiB ceiling', async () => {
  const bytes = Buffer.from('Complete original notice\n');
  const checksum = createHash('sha256').update(bytes).digest('hex');
  const success = await sourceBytes('https://source.invalid/COPYRIGHT', async (_, options) => {
    assert.ok(options.signal instanceof AbortSignal);
    assert.equal(options.signal.aborted, false);
    return { ok: true, body: [bytes] };
  });
  assert.ok(success.equals(bytes));
  let stopped = false;
  await assert.rejects(fetchVerifiedNotice(['https://source.invalid/COPYRIGHT'], checksum,
    (url) => sourceBytes(url, async () => ({ ok: true, body: (async function* () {
      try {
        yield Buffer.alloc(1024 * 1024);
        yield Buffer.from([0]);
        assert.fail('An oversized stream must be stopped before further reading.');
      } finally {
        stopped = true;
      }
    })() }))), (error) => error.code === 'NOTICE_SIZE_LIMIT');
  assert.equal(stopped, true);
  await assert.rejects(fetchVerifiedNotice(['https://source.invalid/COPYRIGHT'], checksum,
    async () => Buffer.alloc(1024 * 1024 + 1)), /size limit/);
});

test('combined licenses, IJG acknowledgment and MPL source availability survive generation', () => {
  const sections = ['MPL-2.0', 'BSD-3-Clause', 'IJG', 'Unicode-3.0']
    .map((id) => `<section data-license="${id}">license</section>`).join('\n');
  const acknowledgment = 'This software is based in part on the work of the Independent JPEG Group.';
  const source = 'https://crates.io/api/v1/crates/acadrust/0.6.3/download';
  const html = `${sections}\n${acknowledgment}\n${source}`;
  assert.doesNotThrow(() => verifyCargoNotices(html));
  for (const id of ['IJG', 'Unicode-3.0', 'BSD-3-Clause']) {
    assert.throws(() => verifyCargoNotices(html.replace(`data-license="${id}"`, '')), /omit/);
  }
  assert.throws(() => verifyCargoNotices(html.replace(acknowledgment, '')), /IJG acknowledgment/);
  assert.throws(() => verifyCargoNotices(html.replace(source, '')), /source-availability/);
});

test('notice output selection rejects ambiguous command arguments', () => {
  assert.ok(outputDirectory([]).endsWith('release-prepared'));
  assert.ok(outputDirectory(['--output', 'custom']).endsWith('custom'));
  assert.throws(() => outputDirectory(['--output']), /Usage/);
  assert.throws(() => outputDirectory(['--output', '--other']), /Usage/);
  assert.throws(() => outputDirectory(['--output', 'one', '--output', 'two']), /Usage/);
});
