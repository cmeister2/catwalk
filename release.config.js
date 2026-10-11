/** Publish the tested Rust crate and matching GitHub release from main. */
export default {
  branches: ['main'],
  repositoryUrl: 'https://github.com/cmeister2/catwalk.git',
  tagFormat: '${version}',
  plugins: [
    ['@semantic-release/commit-analyzer', { preset: 'conventionalcommits' }],
    ['@semantic-release/release-notes-generator', { preset: 'conventionalcommits' }],
    ['@semantic-release/exec', {
      prepareCmd: 'python3 prepare.py ${nextRelease.version} && python3 publish.py ${nextRelease.version}',
    }],
    ['@semantic-release/github', {
      successComment: false,
      failComment: false,
      failTitle: false,
      releasedLabels: false,
      assets: [{
        path: 'target/release-artifacts/catwalk3d.crate',
        name: 'catwalk3d-${nextRelease.version}.crate',
        label: 'Rust crate source',
      }],
    }],
  ],
};
