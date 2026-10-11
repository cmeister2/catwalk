import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmod, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { unzipSync, zipSync } from 'fflate';
import { archiveName, targetDefinition, validateCommit, validateVersion } from './targets.mjs';

/** Maximum admitted ZIP size and combined uncompressed release payload. */
export const MAX_PAYLOAD_BYTES = 256 * 1024 * 1024;

/** Name the single directory containing a target's release payload. */
export function payloadRoot(version, target) {
  return archiveName(version, target).slice(0, -4);
}

/** List the exact files distributed for a target, without accepting arbitrary paths. */
export function payloadFiles(target) {
  const definition = targetDefinition(target);
  return [
    definition.binary, 'README.md', 'LICENSE', 'THIRD_PARTY_LICENSES.html',
    'RUST-LICENSES.html', 'release.json', ...(definition.musl ? ['MUSL-COPYRIGHT'] : []),
  ];
}

/** Hash archive bytes and individual members using the release checksum algorithm. */
export function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

/** Admit nonempty bounded byte arrays before compression or executable extraction. */
function boundedBytes(bytes, label) {
  if (!(bytes instanceof Uint8Array) || bytes.length === 0 || bytes.length > MAX_PAYLOAD_BYTES) {
    throw new Error(`Invalid or oversized release bytes: ${label}`);
  }
  return bytes;
}

/** Compress only the complete allowlisted release payload using a standard ZIP codec. */
export function createArchive({ version, target, files }) {
  const root = payloadRoot(version, target);
  const expected = payloadFiles(target);
  if (!(files instanceof Map) || files.size !== expected.length
    || expected.some((name) => !files.has(name))) {
    throw new Error('Release payload does not contain the exact expected files.');
  }
  const entries = Object.create(null);
  let total = 0;
  for (const name of expected) {
    const bytes = boundedBytes(files.get(name), name);
    total += bytes.length;
    if (total > MAX_PAYLOAD_BYTES) throw new Error('Combined release payload is oversized.');
    entries[`${root}/${name}`] = [bytes, {
      level: 6,
      os: 3,
      attrs: (name === targetDefinition(target).binary ? 0o100755 : 0o100644) * 65536,
    }];
  }
  const archive = boundedBytes(zipSync(entries), 'ZIP archive');
  inspectArchive(archive, { version, target });
  return archive;
}

/** Inspect exact member names and size limits before fflate decompresses each member. */
export function inspectArchive(bytes, { version, target }) {
  boundedBytes(bytes, 'ZIP archive');
  const root = payloadRoot(version, target);
  const expected = new Set(payloadFiles(target).map((name) => `${root}/${name}`));
  const seen = new Map();
  let total = 0;
  const decoded = unzipSync(bytes, {
    filter(file) {
      if (!expected.has(file.name)) throw new Error(`Unexpected ZIP member: ${file.name}`);
      if (seen.has(file.name)) throw new Error(`Duplicate ZIP member: ${file.name}`);
      if (![0, 8].includes(file.compression)) throw new Error('Unsupported ZIP compression.');
      if (!Number.isSafeInteger(file.originalSize) || file.originalSize <= 0
        || file.originalSize > MAX_PAYLOAD_BYTES) {
        throw new Error(`Invalid or oversized ZIP member: ${file.name}`);
      }
      total += file.originalSize;
      if (total > MAX_PAYLOAD_BYTES) throw new Error('Combined ZIP payload is oversized.');
      seen.set(file.name, file.originalSize);
      return true;
    },
  });
  if (seen.size !== expected.size || Object.keys(decoded).length !== expected.size) {
    throw new Error('ZIP archive is missing release payload files.');
  }
  const members = new Map();
  for (const name of expected) {
    const member = decoded[name];
    if (!(member instanceof Uint8Array) || member.length !== seen.get(name)) {
      throw new Error(`ZIP member size does not match its metadata: ${name}`);
    }
    members.set(name, member);
  }
  return members;
}

/** Check architecture and operating system before executing a supposedly native build. */
export function validateNativeHost(target, platform = process.platform, architecture = process.arch) {
  targetDefinition(target);
  const expectedPlatform = target.includes('-linux-') ? 'linux'
    : target.includes('-apple-') ? 'darwin' : 'win32';
  const expectedArchitecture = target.startsWith('aarch64-') ? 'arm64' : 'x64';
  if (platform !== expectedPlatform || architecture !== expectedArchitecture) {
    throw new Error(`Release target ${target} does not match native host ${platform}/${architecture}.`);
  }
}

/** Keep command output bounded and pass arguments without invoking a shell. */
function command(program, args, cwd) {
  return execFileSync(program, args, {
    cwd, encoding: 'utf8', maxBuffer: 1024 * 1024, timeout: 120_000,
  }).trim();
}

/** Reject dynamic musl executables while allowing a statically linked PIE. */
function verifyStaticMusl(binary, cwd) {
  const segments = command('readelf', ['--program-headers', '--wide', binary], cwd);
  const dynamic = command('readelf', ['--dynamic', '--wide', binary], cwd);
  if (/\bINTERP\b/.test(segments) || /\(NEEDED\)/.test(dynamic)) {
    throw new Error('The musl release executable requires a dynamic loader or shared library.');
  }
}

/** Read regular files only after checking the release payload size bound. */
async function readPayloadFile(path) {
  const information = await stat(path);
  if (!information.isFile() || information.size === 0 || information.size > MAX_PAYLOAD_BYTES) {
    throw new Error(`Invalid or oversized release file: ${path}`);
  }
  return boundedBytes(await readFile(path), path);
}

/** Check a smoke render's PNG signature, IHDR dimensions and final IEND chunk. */
function verifySmokePng(bytes) {
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  if (bytes.length < 45 || !bytes.subarray(0, 8).equals(signature)
    || bytes.readUInt32BE(8) !== 13 || bytes.toString('ascii', 12, 16) !== 'IHDR'
    || bytes.readUInt32BE(16) !== 64 || bytes.readUInt32BE(20) !== 64
    || !bytes.subarray(-12).equals(Buffer.from([0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130]))) {
    throw new Error('The packaged executable did not produce the expected 64x64 PNG.');
  }
}

/** Execute the binary extracted from known ZIP bytes, then discard the temporary files. */
async function smokeTest(members, metadata, cwd) {
  const directory = await mkdtemp(join(tmpdir(), 'catwalk-release-'));
  try {
    const binary = join(directory, metadata.binary);
    await writeFile(binary, members.get(`${payloadRoot(metadata.version, metadata.target)}/${metadata.binary}`));
    if (process.platform !== 'win32') await chmod(binary, 0o755);
    if (command(binary, ['--version'], cwd) !== `catwalk ${metadata.version}`) {
      throw new Error('The packaged executable reports the wrong release version.');
    }
    if (!command(binary, ['--list-formats'], cwd).startsWith('Implemented model readers: ')) {
      throw new Error('The packaged executable cannot list its model readers.');
    }
    if (targetDefinition(metadata.target).musl) verifyStaticMusl(binary, cwd);
    const preview = join(directory, 'gear.png');
    command(binary, [join(cwd, 'examples/models/gear.obj'), '--output', preview, '--size', '64', '--samples', '1'], cwd);
    verifySmokePng(await readPayloadFile(preview));
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

/** Package a native build only after its stamped version and extracted executable pass. */
export async function packageBinary(target, { cwd = process.cwd() } = {}) {
  validateNativeHost(target);
  const definition = targetDefinition(target);
  const prepared = join(cwd, 'release-prepared');
  const plan = JSON.parse(await readPayloadFile(join(prepared, 'plan.json')));
  const version = validateVersion(plan.version);
  const commit = validateCommit(plan.commit);
  const rustVersion = validateVersion(plan.rustVersion);
  if (plan.schema !== 1 || typeof plan.releasing !== 'boolean') throw new Error('Invalid prepared release plan.');
  if (command('git', ['rev-parse', 'HEAD'], cwd) !== commit) {
    throw new Error('The source checkout does not match the prepared release commit.');
  }
  const cargo = JSON.parse(command('cargo', ['metadata', '--format-version', '1', '--no-deps', '--locked'], cwd));
  if (cargo.packages.find((entry) => entry.name === 'catwalk3d')?.version !== version) {
    throw new Error('The build manifest does not contain the prepared release version.');
  }
  if (command('rustc', ['--version'], cwd).split(/\s+/)[1] !== rustVersion) {
    throw new Error('The build compiler does not match the prepared release plan.');
  }
  const metadata = { schema: 1, version, commit, rustVersion, target, binary: definition.binary };
  const files = new Map();
  for (const name of payloadFiles(target)) {
    if (name === 'release.json') {
      files.set(name, Buffer.from(`${JSON.stringify(metadata, null, 2)}\n`));
    } else {
      files.set(name, await readPayloadFile(name === definition.binary
        ? join(cwd, 'target', target, 'release', name) : join(prepared, name)));
    }
  }
  const archive = archiveName(version, target);
  const bytes = createArchive({ version, target, files });
  const members = inspectArchive(bytes, { version, target });
  await smokeTest(members, metadata, cwd);
  const checksum = sha256(bytes);
  const sidecar = {
    ...metadata, archive, sha256: checksum,
    files: [...members].map(([path, contents]) => ({ path, sha256: sha256(contents), size: contents.length })),
  };
  const output = join(cwd, 'release-assets');
  await mkdir(output, { recursive: true });
  await writeFile(join(output, archive), bytes);
  await writeFile(join(output, archive.replace(/\.zip$/, '.json')), `${JSON.stringify(sidecar, null, 2)}\n`);
  await writeFile(join(output, `${archive}.sha256`), `${checksum}  ${archive}\n`);
  return sidecar;
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1] ?? '')).href) {
  if (process.argv.length !== 3) {
    console.error('Usage: node .github/release/package.mjs TARGET');
    process.exitCode = 1;
  } else {
    try {
      const result = await packageBinary(process.argv[2]);
      console.log(`Packaged and tested ${result.archive}`);
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
}
