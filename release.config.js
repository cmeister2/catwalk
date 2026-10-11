import { createRequire } from 'node:module';

// Select this project's pinned generator instead of semantic-release's older bundled copy.
const releaseNotesGenerator = createRequire(import.meta.url).resolve(
  '@semantic-release/release-notes-generator'
);

/** Publish the tested Rust crate and matching GitHub release from main. */
export default {
  branches: ['main'],
  repositoryUrl: 'https://github.com/cmeister2/catwalk3d.git',
  tagFormat: '${version}',
  plugins: [
    ['@semantic-release/commit-analyzer', { preset: 'conventionalcommits' }],
    [releaseNotesGenerator, { preset: 'conventionalcommits' }],
    '@semantic-release-cargo/semantic-release-cargo',
    ['@semantic-release/github', {
      successComment: false,
      failComment: false,
      failTitle: false,
      releasedLabels: false,
    }],
  ],
};
