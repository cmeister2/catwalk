// Notices for the C runtime shipped in Rust's self-contained WASI target.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { compilerIdentity, fetchVerifiedNotice, sourceBytes } from '../release/notices.mjs';

const maximumBytes = 1024 * 1024;
const filename = 'WASI-LIBC-LICENSES.txt';

// Rust b940084d's dist-various-2 builder downloads SDK 33.0. SDK tag
// wasi-sdk-33 pins this libc submodule; its musl COPYRIGHT exactly matches
// the existing native 1.2.5 notice. Keep these original bytes qualified.
const knownSdks = new Map([
  ['33.0', {
    commit: '161b3195fc2558d2b1ba3eb9ffae3b2b47407623',
    muslSha256: 'f9bc4423732350eb0b3f7ed7e91d530298476f8fec0c6c427a1c04ade22655af',
    files: [
      { path: 'LICENSE', sha256: '2711a8b5a5cdfef0e639f96c1aca12ae23d7d64a02d0507f1bdf14d2b27bbc3a' },
      { path: 'LICENSE-MIT', sha256: '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3' },
      { path: 'libc-bottom-half/cloudlibc/LICENSE', sha256: 'c8b789cf5a746611e6300a0cc7750dbf92b61912a709d04e639245f7290656d0' },
      { path: 'fts/musl-fts/COPYING', sha256: '55af87e4017668f54467a3380e7ebbac5e672d8c763bfe95e6fc882a6fdc4046' },
      {
        path: 'dlmalloc/src/malloc.c',
        sha256: '9e7ce51d813331ae87ef235f6ca2d5ce2167692e30bb51a173513e3754a67f8f',
        // The complete verified source supplies only its original CC0 notice;
        // no allocator implementation is copied into the distributed site.
        noticeBytes: 269,
        noticeSha256: '80cd40e7a6955355308ece79af9e8dcb3646af20e79c75ac6f3f9bf02536e929',
      },
    ],
  }],
]);

/** Identify the actual SDK download in the pinned Rust distribution builder. */
export function bundledWasiSdk(dockerfile) {
  if (typeof dockerfile !== 'string' ||
      !/^\s*ENV\s+TARGETS[^\n]*[=,]wasm32-wasip1(?:\s|$)/m.test(dockerfile)) {
    throw new Error('Pinned Rust builder does not identify the wasm32-wasip1 target');
  }
  const versions = new Set();
  const pattern = /https:\/\/github\.com\/WebAssembly\/wasi-sdk\/releases\/download\/wasi-sdk-(\d+)\/wasi-sdk-(\d+\.\d+)-x86_64-linux\.tar\.gz/g;
  for (const match of dockerfile.matchAll(pattern)) {
    if (match[1] !== match[2].split('.')[0]) {
      throw new Error('Pinned Rust WASI SDK tag and download version disagree');
    }
    versions.add(match[2]);
  }
  if (versions.size !== 1) throw new Error('Pinned Rust builder has no unambiguous WASI SDK download');
  const [version] = versions;
  if (!knownSdks.has(version)) {
    throw new Error(`WASI SDK ${version} requires qualified original libc notices`);
  }
  return version;
}

/** Resolve original notice URLs, never a moving upstream branch or host libc. */
export function wasiLibcSources(version) {
  const sdk = knownSdks.get(version);
  if (!sdk) throw new Error(`WASI SDK ${version} requires qualified original libc notices`);
  return sdk.files.map(file => ({ ...file,
    url: `https://raw.githubusercontent.com/WebAssembly/wasi-libc/${sdk.commit}/${file.path}`,
  }));
}

/** Require complete bounded UTF-8 before incorporating original notice bytes. */
function text(bytes) {
  if (!Buffer.isBuffer(bytes) || bytes.length > maximumBytes) {
    throw new Error('WASI notice source exceeds the 1 MiB size limit');
  }
  const value = bytes.toString('utf8');
  if (!Buffer.from(value).equals(bytes)) throw new Error('WASI notice source is not UTF-8');
  return value;
}

/** Preserve an original verified file, or its separately checked notice prefix. */
export function renderOriginalWasiNotice(source, bytes) {
  text(bytes);
  if (createHash('sha256').update(bytes).digest('hex') !== source.sha256) {
    throw new Error(`Original WASI libc notice checksum differs: ${source.path}`);
  }
  const notice = source.noticeBytes === undefined ? bytes : bytes.subarray(0, source.noticeBytes);
  if (source.noticeSha256 && createHash('sha256').update(notice).digest('hex') !== source.noticeSha256) {
    throw new Error('Original dlmalloc notice prefix checksum differs');
  }
  return Buffer.concat([
    Buffer.from(`===== ${source.path}${source.noticeBytes ? ' (original notice prefix)' : ''} =====\n${source.url}\n\n`),
    notice, Buffer.from('\n\n'),
  ]);
}

/** Preserve full notices and the separately identified allocator notice exactly. */
export function renderWasiLibcNotices(version, contents, musl) {
  const sdk = knownSdks.get(version);
  const sources = wasiLibcSources(version);
  if (!Array.isArray(contents) || contents.length !== sources.length) {
    throw new Error('WASI libc notices require every qualified original file');
  }
  text(musl);
  if (createHash('sha256').update(musl).digest('hex') !== sdk.muslSha256) {
    throw new Error('Prepared MUSL-COPYRIGHT differs from the WASI libc original');
  }
  const output = [Buffer.from(
    `Rust self-contained WASI C runtime\nWASI SDK ${version}; wasi-libc ${sdk.commit}\n` +
    'The MIT option is used for wasi-libc; third-party portions retain their original terms.\n' +
    'The musl portion is covered by the accompanying byte-identical MUSL-COPYRIGHT.\n\n',
  )];
  for (const [index, source] of sources.entries()) {
    output.push(renderOriginalWasiNotice(source, contents[index]));
  }
  return Buffer.concat(output);
}

/** Bind runtime notices to the actual compiler and its prepared release identity. */
export async function generateWasiLibcNotices(output, preparedDirectory, {
  verboseVersion, load = sourceBytes,
} = {}) {
  const plan = JSON.parse(await readFile(join(preparedDirectory, 'plan.json'), 'utf8'));
  const actual = verboseVersion ?? execFileSync('rustc', ['-vV'], {
    encoding: 'utf8', maxBuffer: maximumBytes, stdio: ['ignore', 'pipe', 'inherit'],
  });
  const compiler = compilerIdentity(plan, actual);
  const url = `https://raw.githubusercontent.com/rust-lang/rust/${compiler.commit}/src/ci/docker/host-x86_64/dist-various-2/Dockerfile`;
  const version = bundledWasiSdk(text(await load(url)));
  const sources = wasiLibcSources(version);
  const [musl, ...contents] = await Promise.all([
    readFile(join(preparedDirectory, 'MUSL-COPYRIGHT')),
    ...sources.map(source => fetchVerifiedNotice([source.url], source.sha256, load)),
  ]);
  const bytes = renderWasiLibcNotices(version, contents, musl);
  await mkdir(output, { recursive: true });
  const temporary = join(output, `${filename}.tmp`);
  try {
    await writeFile(temporary, bytes);
    await rename(temporary, join(output, filename));
  } finally {
    await rm(temporary, { force: true });
  }
  return { filename, sdkVersion: version, wasiLibcCommit: knownSdks.get(version).commit,
    compilerVersion: compiler.version, compilerCommit: compiler.commit };
}
