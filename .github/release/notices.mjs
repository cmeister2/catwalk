// Generate distributable notices from the prepared release and its Rust toolchain.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repository = fileURLToPath(new URL('../../', import.meta.url));
const aboutVersion = 'cargo-about 0.9.2';

// The handwritten wrapper is MIT; bundled OpenUSD schemas and generated schema
// code have separate terms.
// cargo-about 0.9.2's custom LicenseRef text filter drops matching clarified
// files (src/generate.rs:158-163), so preserve these verified originals explicitly.
const schemaNotices = new Map([
  ['0.8.0', {
    commit: '9c007a00fd2ca243a7b6812dd0bf34eca01ee878',
    files: [
      {
        path: 'third-party/LICENSE',
        sha256: 'ce5cb334bff4a1c576be24069eec3e49ff6eeba1ccccc45c33607655d347ae75',
      },
      {
        path: 'third-party/NOTICE.txt',
        sha256: '2949534cb3ef969acbaf52342aabea7adff97e1e0d09e44b5538c3a54625155b',
      },
    ],
  }],
]);
const schemaLicenseId = 'LicenseRef-Tomorrow-Open-Source-Technology-License-1.0';
const maximumNoticeBytes = 1024 * 1024;

// These are upstream notice bytes, not a copy of the dependency or its source.
const muslNotices = new Map([
  ['1.2.5', {
    url: 'https://git.musl-libc.org/cgit/musl/plain/COPYRIGHT?h=v1.2.5',
    // Exact release-1.2.5 mirror bytes were verified against the primary notice.
    fallbackUrl: 'https://raw.githubusercontent.com/ifduyue/musl/0784374d561435f7c787a555aeab8ede699ed298/COPYRIGHT',
    sha256: 'f9bc4423732350eb0b3f7ed7e91d530298476f8fec0c6c427a1c04ade22655af',
  }],
]);

/** Read the single output-directory option without accepting unknown arguments. */
export function outputDirectory(args, cwd = repository) {
  if (args.length === 0) return resolve(cwd, 'release-prepared');
  if (args.length !== 2 || args[0] !== '--output' || !args[1] || args[1].startsWith('--')) {
    throw new Error('Usage: node .github/release/notices.mjs [--output DIRECTORY]');
  }
  return resolve(cwd, args[1]);
}

/** Bind source/license collection to the exact compiler in the release plan. */
export function compilerIdentity(plan, verboseVersion) {
  const version = /^release: (.+)$/m.exec(verboseVersion)?.[1];
  const commit = /^commit-hash: ([a-f0-9]{40})$/m.exec(verboseVersion)?.[1];
  if (plan.schema !== 1 || typeof plan.rustVersion !== 'string' || !version || !commit) {
    throw new Error('Release plan or rustc identity is incomplete');
  }
  if (version !== plan.rustVersion) {
    throw new Error(`Notice toolchain ${version} differs from planned Rust ${plan.rustVersion}`);
  }
  return { version, commit };
}

/** Resolve the libc version actually built by the pinned Rust musl script. */
export function bundledMuslVersion(script) {
  const versions = new Set([...script.matchAll(/\bMUSL_VER=(\d+\.\d+\.\d+)\b/g)].map((m) => m[1]));
  if (versions.size !== 1) throw new Error('Rust musl build script has no unambiguous MUSL_VER');
  const [version] = versions;
  if (!muslNotices.has(version)) {
    throw new Error(`Bundled musl ${version} requires a verified upstream copyright notice`);
  }
  return version;
}

/** Refuse truncated, substituted or normalized musl copyright bytes. */
export function verifyMuslCopyright(version, bytes) {
  const notice = muslNotices.get(version);
  if (!notice || createHash('sha256').update(bytes).digest('hex') !== notice.sha256) {
    throw new Error(`Bundled musl ${version} copyright checksum differs from the upstream notice`);
  }
}

/** Fetch a complete original notice, using its verified mirror only after source unavailability. */
export async function fetchVerifiedNotice(sources, checksum, load = sourceBytes) {
  if (!Array.isArray(sources) || sources.length === 0 || sources.length > 2) {
    throw new Error('A verified notice requires one or two bounded sources');
  }
  const failures = [];
  for (const source of sources) {
    let bytes;
    try {
      bytes = await load(source);
    } catch (error) {
      if (error.code === 'NOTICE_SIZE_LIMIT') throw error;
      failures.push(error);
      continue;
    }
    // Returned substitutions or oversized content are errors, never a reason
    // to conceal an upstream change by trying the mirror instead.
    if (!Buffer.isBuffer(bytes) || bytes.length > maximumNoticeBytes) {
      throw new Error('Original notice exceeds the 1 MiB size limit');
    }
    if (createHash('sha256').update(bytes).digest('hex') !== checksum) {
      throw new Error('Original notice checksum differs');
    }
    return bytes;
  }
  throw new AggregateError(failures, 'All verified original notice sources are unavailable');
}

/** Keep primary and immutable-mirror musl downloads under the original copyright checksum. */
export async function fetchMuslCopyright(version, load = sourceBytes) {
  const notice = muslNotices.get(version);
  if (!notice) throw new Error(`Bundled musl ${version} requires a verified upstream copyright notice`);
  const bytes = await fetchVerifiedNotice([notice.url, notice.fallbackUrl], notice.sha256, load);
  verifyMuslCopyright(version, bytes);
  return bytes;
}

/** Keep mandatory combined-license clauses and executable-distribution notices. */
export function verifyCargoNotices(html, acadrustVersion) {
  for (const id of ['MPL-2.0', 'BSD-3-Clause', 'IJG', 'Unicode-3.0']) {
    if (!html.includes(`data-license="${id}"`)) throw new Error(`Generated notices omit ${id}`);
  }
  if (!html.includes('This software is based in part on the work of the Independent JPEG Group.')) {
    throw new Error('Generated notices omit the IJG acknowledgment');
  }
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(acadrustVersion)) {
    throw new Error('The resolved acadrust version is invalid');
  }
  const source = `https://crates.io/api/v1/crates/acadrust/${acadrustVersion}/download`;
  const sections = [...html.matchAll(/<section\b[^>]*\bdata-license="MPL-2\.0"[^>]*>([\s\S]*?)<\/section>/g)];
  const linked = sections.some((section) => [...section[1].matchAll(/<li\b[^>]*>([\s\S]*?)<\/li>/g)]
    .some((item) => item[1].trimStart().startsWith(`acadrust ${acadrustVersion}:`)
      && [...item[1].matchAll(/<a\b[^>]*\bhref="([^"]+)"[^>]*>/g)]
        .some((anchor) => anchor[1] === source)));
  if (!linked) {
    throw new Error('Generated notices omit the pinned acadrust source-availability link');
  }
}

/** Bind version-specific source links and schema notices to published locked packages. */
export function lockedNoticeVersions(lockfile) {
  const versions = {};
  for (const name of ['acadrust', 'openusd-schemas']) {
    const matches = lockfile.split(/^\[\[package\]\]\s*$/m).slice(1)
      .filter((block) => new RegExp(`^name\\s*=\\s*"${name}"\\s*$`, 'm').test(block));
    if (matches.length !== 1) throw new Error(`Release notices require one resolved ${name} package`);
    const version = /^version\s*=\s*"([^"]+)"\s*$/m.exec(matches[0])?.[1];
    const source = /^source\s*=\s*"([^"]+)"\s*$/m.exec(matches[0])?.[1];
    if (!version || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)
      || source !== 'registry+https://github.com/rust-lang/crates.io-index') {
      throw new Error(`Release notices require a published crates.io ${name} package`);
    }
    versions[name] = version;
  }
  return versions;
}

/** Resolve immutable notice URLs for the exact published schema package. */
export function schemaNoticeSources(version) {
  const notice = schemaNotices.get(version);
  if (!notice) throw new Error(`openusd-schemas ${version} requires verified original license and notice bytes`);
  return notice.files.map((file) => ({ ...file,
    url: `https://raw.githubusercontent.com/mxpv/openusd/${notice.commit}/crates/openusd-schemas/${file.path}`,
  }));
}

/** Escape complete verified upstream notice text without changing its displayed bytes. */
export function renderVerifiedNotice(file, bytes) {
  if (!Buffer.isBuffer(bytes) || bytes.length > maximumNoticeBytes
    || createHash('sha256').update(bytes).digest('hex') !== file.sha256) {
    throw new Error('Original third-party notice checksum or size differs');
  }
  const text = bytes.toString('utf8');
  if (!Buffer.from(text).equals(bytes)) throw new Error('Original third-party notice is not UTF-8');
  const escape = (value) => value.replace(/[&<>"']/g,
    (character) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]);
  return `<h3>${escape(file.path)}</h3>\n<p><a href="${escape(file.url)}">Original upstream notice</a></p>\n<pre data-notice="${escape(file.path)}" data-sha256="${file.sha256}">${escape(text)}</pre>`;
}

/** Include both original schema terms and attribution, refusing partial or substituted bytes. */
export function appendSchemaNotices(html, version, contents) {
  const sources = schemaNoticeSources(version);
  if (!Array.isArray(contents) || contents.length !== sources.length
    || !/<\/html>\s*$/i.test(html) || html.includes(`data-license="${schemaLicenseId}"`)) {
    throw new Error('Schema notice appendix needs one complete original license/notice pair and HTML document');
  }
  const texts = sources.map((source, index) => renderVerifiedNotice(source, contents[index])).join('\n');
  const appendix = `<section data-license="${schemaLicenseId}">\n<h2>Bundled OpenUSD schemas and generated schema code</h2>\n<p>openusd-schemas ${version} bundles OpenUSD schemas and generated schema code covered by the Tomorrow Open Source Technology License 1.0. The handwritten crate implementation has separate MIT terms above. These are the complete original license and attribution bytes from the published package.</p>\n<p><a href="https://crates.io/api/v1/crates/openusd-schemas/${version}/download">Exact published package source</a></p>\n${texts}\n</section>\n`;
  return html.replace(/<\/html>\s*$/i, () => `${appendix}</html>\n`);
}

/** Fetch a bounded public source file without credentials or local configuration. */
export async function sourceBytes(url, fetchSource = fetch) {
  const response = await fetchSource(url, { signal: AbortSignal.timeout(30_000) });
  if (!response.ok) throw new Error(`Cannot retrieve notice source: ${url} (${response.status})`);
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > maximumNoticeBytes) {
      const error = new Error(`Notice source exceeds 1 MiB: ${url}`);
      error.code = 'NOTICE_SIZE_LIMIT';
      throw error;
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

/** Execute installed standard tools; command output never contains credentials. */
function command(executable, args) {
  return execFileSync(executable, args, {
    cwd: repository, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'inherit'],
  }).trim();
}

/** Generate all three notice files after validating the prepared compiler. */
export async function generateNotices(output) {
  const plan = JSON.parse(await readFile(join(output, 'plan.json'), 'utf8'));
  const versions = lockedNoticeVersions(await readFile(join(repository, 'Cargo.lock'), 'utf8'));
  const schemaSources = schemaNoticeSources(versions['openusd-schemas']);
  const compiler = compilerIdentity(plan, command('rustc', ['-vV']));
  if (command('cargo-about', ['--version']) !== aboutVersion) {
    throw new Error(`Notice generation requires ${aboutVersion}`);
  }
  const sysroot = command('rustc', ['--print', 'sysroot']);
  const rustNotice = join(sysroot, 'share', 'doc', 'rust', 'COPYRIGHT-library.html');
  const rustHtml = await readFile(rustNotice, 'utf8');
  if (!rustHtml.includes('Rust Project')) throw new Error('Rust library copyright notice is incomplete');

  const scriptUrl = `https://raw.githubusercontent.com/rust-lang/rust/${compiler.commit}/src/ci/docker/scripts/musl-toolchain.sh`;
  const [script, ...schemaContents] = await Promise.all([
    sourceBytes(scriptUrl), ...schemaSources.map((source) => sourceBytes(source.url)),
  ]);
  const muslVersion = bundledMuslVersion(script.toString('utf8'));
  const musl = await fetchMuslCopyright(muslVersion);

  await mkdir(output, { recursive: true });
  const temporary = join(output, 'THIRD_PARTY_LICENSES.html.tmp');
  try {
    command('cargo-about', [
      'generate', '--locked', '--fail', '--config', join(repository, '.github', 'release', 'about.toml'),
      '--manifest-path', join(repository, 'Cargo.toml'), '--output-file', temporary,
      join(repository, '.github', 'release', 'licenses.hbs'),
    ]);
    const html = appendSchemaNotices(await readFile(temporary, 'utf8'),
      versions['openusd-schemas'], schemaContents);
    verifyCargoNotices(html, versions.acadrust);
    await writeFile(temporary, html);
    await copyFile(rustNotice, join(output, 'RUST-LICENSES.html'));
    await writeFile(join(output, 'MUSL-COPYRIGHT'), musl);
    await rename(temporary, join(output, 'THIRD_PARTY_LICENSES.html'));
  } finally {
    await rm(temporary, { force: true });
  }
  console.log(`Generated release notices for Rust ${compiler.version}, bundled musl ${muslVersion}`);
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  await generateNotices(outputDirectory(process.argv.slice(2)));
}
