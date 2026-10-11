import assert from 'node:assert/strict';
import test from 'node:test';
import {
  bundledMuslVersion, compilerIdentity, outputDirectory, verifyCargoNotices, verifyMuslCopyright,
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
