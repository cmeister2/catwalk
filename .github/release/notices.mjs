// Generate distributable notices from the prepared release and its Rust toolchain.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repository = fileURLToPath(new URL('../../', import.meta.url));
const aboutVersion = 'cargo-about 0.9.2';
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
export function verifyCargoNotices(html) {
  for (const id of ['MPL-2.0', 'BSD-3-Clause', 'IJG', 'Unicode-3.0']) {
    if (!html.includes(`data-license="${id}"`)) throw new Error(`Generated notices omit ${id}`);
  }
  if (!html.includes('This software is based in part on the work of the Independent JPEG Group.')) {
    throw new Error('Generated notices omit the IJG acknowledgment');
  }
  if (!html.includes('https://crates.io/api/v1/crates/acadrust/0.6.3/download')) {
    throw new Error('Generated notices omit the pinned acadrust source-availability link');
  }
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
  const compiler = compilerIdentity(plan, command('rustc', ['-vV']));
  if (command('cargo-about', ['--version']) !== aboutVersion) {
    throw new Error(`Notice generation requires ${aboutVersion}`);
  }
  const sysroot = command('rustc', ['--print', 'sysroot']);
  const rustNotice = join(sysroot, 'share', 'doc', 'rust', 'COPYRIGHT-library.html');
  const rustHtml = await readFile(rustNotice, 'utf8');
  if (!rustHtml.includes('Rust Project')) throw new Error('Rust library copyright notice is incomplete');

  const scriptUrl = `https://raw.githubusercontent.com/rust-lang/rust/${compiler.commit}/src/ci/docker/scripts/musl-toolchain.sh`;
  const muslVersion = bundledMuslVersion((await sourceBytes(scriptUrl)).toString('utf8'));
  const musl = await fetchMuslCopyright(muslVersion);

  await mkdir(output, { recursive: true });
  const temporary = join(output, 'THIRD_PARTY_LICENSES.html.tmp');
  try {
    command('cargo-about', [
      'generate', '--locked', '--fail', '--config', join(repository, '.github', 'release', 'about.toml'),
      '--manifest-path', join(repository, 'Cargo.toml'), '--output-file', temporary,
      join(repository, '.github', 'release', 'licenses.hbs'),
    ]);
    verifyCargoNotices(await readFile(temporary, 'utf8'));
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
