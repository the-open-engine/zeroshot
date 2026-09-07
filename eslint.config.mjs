import prettierConfig from 'eslint-config-prettier';

export default [
  {
    ignores: [
      '.opcore/graph/**',
      'docs/zeroshot-cli.html',
      'node_modules/**',
      'protocol/**',
      'sdks/python/**',
      'target/**',
    ],
  },
  {
    files: [
      '.github/**/*.js',
      'scripts/**/*.js',
      'npm/zeroshot/**/*.js',
      'tests/tooling/**/*.js',
      'commitlint.config.js',
    ],
    languageOptions: {
      ecmaVersion: 'latest',
      sourceType: 'commonjs',
    },
    rules: {
      'no-constant-condition': 'error',
      'no-control-regex': 'error',
      'no-dupe-keys': 'error',
      'no-unreachable': 'error',
      'no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
      'no-undef': 'off',
      eqeqeq: ['error', 'always'],
    },
  },
  prettierConfig,
];
