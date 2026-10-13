import {
  WASI, File, OpenFile, Directory, OpenDirectory, PreopenDirectory, ConsoleStdout, wasi,
} from './vendor/wasi.js';
import { MAX_FILE_BYTES, MAX_TOTAL_BYTES, MAX_OUTPUT_BYTES, MAX_FILES } from './config.js';

const MAX_STDERR_BYTES = 64 * 1024;
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

export class RenderError extends Error {
  constructor(message, { exitCode, stderr = '', cause } = {}) {
    super(message, { cause });
    this.name = 'RenderError';
    this.exitCode = exitCode;
    this.stderr = stderr;
  }
}

/** Preserve authored relative names while refusing aliases and directory escapes. */
function relativePath(name) {
  if (typeof name !== 'string' || name.length === 0 || name.length > 4096 ||
      /[\\:\u0000-\u001f\u007f]/u.test(name) || /[\ud800-\udfff]/u.test(name)) {
    throw new RenderError('Each file needs a valid relative filename.');
  }
  const parts = name.split('/');
  if (parts.some(part => part === '' || part === '.' || part === '..')) {
    throw new RenderError('File paths must be relative and cannot contain traversal or empty components.');
  }
  return parts;
}

function fileBytes(bytes) {
  if (bytes instanceof ArrayBuffer) return new Uint8Array(bytes);
  if (ArrayBuffer.isView(bytes) && bytes.buffer instanceof ArrayBuffer) {
    return new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
  throw new RenderError('File contents must be an ArrayBuffer or byte view.');
}

function validateFiles(files) {
  if (!Array.isArray(files) || files.length === 0 || files.length > MAX_FILES) {
    throw new RenderError(`Provide between 1 and ${MAX_FILES} input and companion files.`);
  }
  const names = new Set();
  let total = 0;
  return files.map(file => {
    const parts = relativePath(file?.name);
    if (names.has(file.name)) throw new RenderError(`Duplicate file path: ${file.name}`);
    names.add(file.name);
    const bytes = fileBytes(file.bytes);
    if (bytes.byteLength > MAX_FILE_BYTES) {
      throw new RenderError(`File exceeds the 64 MiB limit: ${file.name}`);
    }
    total += bytes.byteLength;
    if (total > MAX_TOTAL_BYTES) throw new RenderError('Files exceed the combined 128 MiB limit.');
    return { name: file.name, parts, bytes };
  });
}

/** These memory inodes are never symlinks. Rust canonicalize probes readlink. */
function nonSymlinkReadlink(path) {
  const { ret, inode_obj: inode } = this.path_lookup(path, 0);
  return { ret: inode === null ? ret : wasi.ERRNO_INVAL, data: null };
}

class MemoryOpenDirectory extends OpenDirectory {
  path_readlink(path) {
    return nonSymlinkReadlink.call(this, path);
  }
}

class MemoryDirectory extends Directory {
  path_open() {
    return { ret: wasi.ERRNO_SUCCESS, fd_obj: new MemoryOpenDirectory(this) };
  }
}

class MemoryRoot extends PreopenDirectory {
  constructor(contents) {
    super('/', contents);
    this.dir = new MemoryDirectory(contents);
  }
  path_readlink(path) {
    return nonSymlinkReadlink.call(this, path);
  }
}

function inputDirectory(entries) {
  const input = new MemoryDirectory(new Map());
  for (const { name, parts, bytes } of entries) {
    let directory = input;
    for (const part of parts.slice(0, -1)) {
      let child = directory.contents.get(part);
      if (child === undefined) {
        child = new MemoryDirectory(new Map());
        child.parent = directory;
        directory.contents.set(part, child);
      }
      if (!(child instanceof MemoryDirectory)) {
        throw new RenderError(`A file collides with a directory: ${name}`);
      }
      directory = child;
    }
    const basename = parts.at(-1);
    if (directory.contents.has(basename)) {
      throw new RenderError(`A file collides with a directory: ${name}`);
    }
    // Passing the exact ArrayBuffer avoids File's extra typed-array copy.
    const buffer = bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength
      ? bytes.buffer : bytes.slice().buffer;
    directory.contents.set(basename, new File(buffer, { readonly: true }));
  }
  return new MemoryRoot(new Map([['input', input]]));
}

/** A fresh memory-only preopen, also used to test the WASI filesystem adapter. */
export function createInputDirectory(files) {
  return inputDirectory(validateFiles(files));
}

function optionsArgs(options) {
  const { size = 256, transparent = false, mode = 'auto' } = options ?? {};
  if (!Number.isInteger(size) || size < 1 || size > 4096) {
    throw new RenderError('Thumbnail size must be an integer from 1 to 4096.');
  }
  if (typeof transparent !== 'boolean' || !['auto', 'geometry'].includes(mode)) {
    throw new RenderError('Invalid thumbnail options.');
  }
  const args = ['--output', '-', '--size', String(size)];
  if (transparent) args.push('--transparent');
  if (mode === 'geometry') args.push('--render-geometry');
  return args;
}

/** Execute the CLI with no environment or host files, releasing its instance afterward. */
export async function runCatwalk(module, files, input, options = {}) {
  relativePath(input);
  const entries = validateFiles(files);
  if (!entries.some(file => file.name === input)) {
    throw new RenderError('The selected input file is missing from the provided files.');
  }
  const args = ['catwalk', `/input/${input}`, ...optionsArgs(options)];
  const root = inputDirectory(entries);
  const stdout = [];
  const stderr = [];
  let outputSize = 0;
  let errorSize = 0;
  let errorTruncated = false;
  const output = new ConsoleStdout(bytes => {
    if (outputSize + bytes.byteLength > MAX_OUTPUT_BYTES) {
      throw new RenderError('Rendered PNG exceeds the 64 MiB output limit.');
    }
    stdout.push(bytes.slice());
    outputSize += bytes.byteLength;
  });
  const errors = new ConsoleStdout(bytes => {
    const keep = Math.min(bytes.byteLength, MAX_STDERR_BYTES - errorSize);
    if (keep > 0) stderr.push(bytes.slice(0, keep));
    errorSize += keep;
    if (keep < bytes.byteLength) errorTruncated = true;
  });
  const wasiInstance = new WASI(args, [], [
    new OpenFile(new File(new ArrayBuffer(0), { readonly: true })), output, errors, root,
  ], { debug: false });
  // Shim 0.4.2 counts UTF-16 code units, but args_get writes UTF-8 bytes.
  // Correct the advertised allocation size for ordinary Unicode filenames.
  const argumentBytes = args.reduce((size, arg) => size + new TextEncoder().encode(arg).length + 1, 0);
  wasiInstance.wasiImport.args_sizes_get = (countPointer, bytesPointer) => {
    const memory = new DataView(wasiInstance.inst.exports.memory.buffer);
    memory.setUint32(countPointer, args.length, true);
    memory.setUint32(bytesPointer, argumentBytes, true);
    return wasi.ERRNO_SUCCESS;
  };
  let exitCode;
  let failure;
  try {
    const instance = await WebAssembly.instantiate(module, {
      wasi_snapshot_preview1: wasiInstance.wasiImport,
    });
    exitCode = wasiInstance.start(instance);
  } catch (error) {
    failure = error;
  }
  const text = new TextDecoder().decode(concatenate(stderr, errorSize)) +
    (errorTruncated ? '\n[stderr truncated]' : '');
  if (failure) {
    if (failure instanceof RenderError) {
      failure.stderr = text;
      throw failure;
    }
    throw new RenderError(`Rendering stopped: ${failure.message ?? String(failure)}`, {
      stderr: text, cause: failure,
    });
  }
  if (exitCode !== 0) {
    const message = exitCode === 3
      ? 'This document has no supported saved preview or geometry fallback.'
      : `Rendering failed (exit ${exitCode})${text.trim() ? `: ${text.trim()}` : '.'}`;
    throw new RenderError(message, { exitCode, stderr: text });
  }
  const png = concatenate(stdout, outputSize);
  if (png.length < PNG_SIGNATURE.length || PNG_SIGNATURE.some((byte, i) => png[i] !== byte)) {
    throw new RenderError('The renderer did not return a PNG image.', { stderr: text });
  }
  return { png, stderr: text, exitCode: 0 };
}

function concatenate(chunks, length) {
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}
