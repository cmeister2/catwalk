/** Publish the tested Rust crate and matching GitHub release from main. */
export default {
  branches: ['main'],
  repositoryUrl: 'https://github.com/cmeister2/catwalk3d.git',
  tagFormat: '${version}',
  plugins: [
    ['@semantic-release/commit-analyzer', { preset: 'conventionalcommits' }],
    ['@semantic-release/release-notes-generator', { preset: 'conventionalcommits' }],
    '@semantic-release-cargo/semantic-release-cargo',
    ['@semantic-release/github', {
      successComment: false,
      failComment: false,
      failTitle: false,
      releasedLabels: false,
    }],
  ],
};
