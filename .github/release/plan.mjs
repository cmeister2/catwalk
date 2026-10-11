import { execFileSync } from 'node:child_process';
import { appendFile, copyFile, mkdir, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import semanticRelease from 'semantic-release';
import releaseConfig from '../../release.config.js';
import { TARGETS, validateCommit, validateVersion } from './targets.mjs';

const require = createRequire(import.meta.url);

/** Retain the configured analysis plugins without loading publishing credentials. */
export function planningPlugins(config) {
  return ['@semantic-release/commit-analyzer', '@semantic-release/release-notes-generator'].map(
    (name) => {
      const entry = config.plugins.find((plugin) => {
        const path = Array.isArray(plugin) ? plugin[0] : plugin;
        return path === name || path === require.resolve(name);
      });
      if (!entry) throw new Error(`Missing release planning plugin: ${name}`);
      return entry;
    }
  );
}

/** Read the crate version without changing the manifest or dependency resolution. */
function crateVersion(cwd, env) {
  const metadata = JSON.parse(execFileSync('cargo', [
    'metadata', '--format-version', '1', '--no-deps', '--locked',
  ], { cwd, env, encoding: 'utf8' }));
  const crate = metadata.packages.find((entry) => entry.name === 'catwalk3d');
  if (!crate) throw new Error('The checkout does not contain the catwalk3d crate.');
  return validateVersion(crate.version);
}

/** Plan using the installed release engine and stamp with its standard Cargo plugin. */
export async function createPlan({
  cwd = process.cwd(), release = false, config = releaseConfig,
  env = process.env, stdout = process.stdout, stderr = process.stderr,
  output = env.GITHUB_OUTPUT,
} = {}) {
  const commit = validateCommit(execFileSync('git', ['rev-parse', 'HEAD'], {
    cwd, env, encoding: 'utf8',
  }).trim());
  const rustVersion = validateVersion(execFileSync('rustc', ['--version'], {
    cwd, env, encoding: 'utf8',
  }).trim().split(/\s+/)[1]);
  let version = crateVersion(cwd, env);
  let releasing = false;
  if (release) {
    const result = await semanticRelease({
      ...config, dryRun: true, plugins: planningPlugins(config),
    }, { cwd, env, stdout, stderr });
    if (result?.nextRelease) {
      if (result.nextRelease.gitHead !== commit) {
        throw new Error('The planned release does not match the producing checkout.');
      }
      version = validateVersion(result.nextRelease.version);
      const previousCwd = process.cwd();
      try {
        // This plugin's native implementation uses the process working directory.
        process.chdir(cwd);
        require('@semantic-release-cargo/semantic-release-cargo').prepare({}, {
          nextRelease: result.nextRelease,
        });
      } finally {
        process.chdir(previousCwd);
      }
      if (crateVersion(cwd, env) !== version) {
        throw new Error('Cargo preparation did not set the planned crate version.');
      }
      releasing = true;
    }
  }
  const plan = { schema: 1, version, commit, releasing, rustVersion };
  const prepared = join(cwd, 'release-prepared');
  await mkdir(prepared, { recursive: true });
  await copyFile(join(cwd, 'Cargo.toml'), join(prepared, 'Cargo.toml'));
  await copyFile(join(cwd, 'Cargo.lock'), join(prepared, 'Cargo.lock'));
  await writeFile(join(prepared, 'plan.json'), `${JSON.stringify(plan, null, 2)}\n`);
  if (output) {
    await appendFile(output, [
      `version=${version}`, `releasing=${releasing}`, `commit=${commit}`,
      `rust-version=${rustVersion}`, `matrix=${JSON.stringify({ include: TARGETS })}`, '',
    ].join('\n'));
  }
  return plan;
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1] ?? '')).href) {
  const args = process.argv.slice(2);
  if (args.length !== 1 || !['--ci', '--release'].includes(args[0])) {
    console.error('Usage: node .github/release/plan.mjs --ci|--release');
    process.exitCode = 1;
  } else {
    try {
      await createPlan({ release: args[0] === '--release' });
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
}
