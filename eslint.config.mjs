import js from '@eslint/js';
import unusedImports from 'eslint-plugin-unused-imports';
import prettierConfig from 'eslint-config-prettier';

export default [
  js.configs.recommended,
  {
    plugins: {
      'unused-imports': unusedImports,
    },
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'module',
      globals: {
        // Node.js globals
        console: 'readonly',
        process: 'readonly',
        Buffer: 'readonly',
        __dirname: 'readonly',
        __filename: 'readonly',
        module: 'readonly',
        require: 'readonly',
        exports: 'readonly',
        global: 'readonly',
        setTimeout: 'readonly',
        clearTimeout: 'readonly',
        setInterval: 'readonly',
        clearInterval: 'readonly',
        setImmediate: 'readonly',
        clearImmediate: 'readonly',
      },
    },
    rules: {
      // Dead code detection - AGGRESSIVE
      'no-unused-vars': 'off', // Disabled in favor of unused-imports plugin
      'unused-imports/no-unused-imports': 'error',
      'unused-imports/no-unused-vars': [
        'error',
        {
          vars: 'all',
          varsIgnorePattern: '^_',
          args: 'after-used',
          argsIgnorePattern: '^_',
        },
      ],
      'no-unreachable': 'error',
      'no-unreachable-loop': 'error',
      'no-constant-condition': ['error', { checkLoops: false }],

      // Variables
      'no-undef': 'error',
      'no-shadow': ['error', { builtinGlobals: false }],
      'no-use-before-define': ['error', { functions: false }],

      // Best practices
      eqeqeq: ['error', 'always'],
      'no-eval': 'error',
      'no-implied-eval': 'error',
      'no-new-func': 'error',
      'no-return-await': 'error',
      'require-await': 'error',
      'no-throw-literal': 'error',
      'prefer-promise-reject-errors': 'error',

      // Code quality
      complexity: ['warn', 20],
      'max-depth': ['warn', 4],
      'max-nested-callbacks': ['warn', 4],
      'max-params': ['warn', 5],
      'max-lines-per-function': ['warn', { max: 150, skipBlankLines: true, skipComments: true }],

      // Security
      'no-unsafe-optional-chaining': 'error',
      'no-prototype-builtins': 'error',

      // Dangerous fallbacks - FORBIDDEN
      'no-restricted-syntax': [
        'error',
        {
          selector:
            "LogicalExpression[operator='||'][right.value=/localhost|127\\.0\\.0|^\\d{4,5}$/]",
          message: 'FORBIDDEN: Dangerous fallback. Throw error if missing instead.',
        },
        {
          // Catches: const { exec } = require('child_process')
          selector:
            "VariableDeclarator[init.callee.name='require'][init.arguments.0.value='child_process'] > ObjectPattern > Property[key.name='exec']",
          message:
            "FORBIDDEN: Direct exec() from child_process can hang forever. Use require('./lib/safe-exec') instead.",
        },
        {
          // Catches: const { execSync } = require('child_process')
          selector:
            "VariableDeclarator[init.callee.name='require'][init.arguments.0.value='child_process'] > ObjectPattern > Property[key.name='execSync']",
          message:
            "FORBIDDEN: Direct execSync() from child_process can hang forever. Use require('./lib/safe-exec') instead.",
        },
      ],

      // Style (keep existing)
      'no-console': 'off',
      'no-case-declarations': 'off',
    },
  },
  {
    files: ['tests/**/*.js'],
    languageOptions: {
      globals: {
        describe: 'readonly',
        it: 'readonly',
        before: 'readonly',
        after: 'readonly',
        beforeEach: 'readonly',
        afterEach: 'readonly',
      },
    },
  },
  {
    // Allow direct child_process in safe-exec wrapper, tests, CLI, and lib/
    // - safe-exec.js: IS the wrapper
    // - tests/: Need to test subprocess behavior directly
    // - cli/: Has its own timeout handling for user-facing commands
    // - lib/: Utility code with specific timeout requirements
    files: ['src/lib/safe-exec.js', 'tests/**/*.js', 'cli/**/*.js', 'lib/**/*.js'],
    rules: {
      'no-restricted-syntax': [
        'error',
        // Keep the dangerous fallback rule, just disable the exec rule
        {
          selector:
            "LogicalExpression[operator='||'][right.value=/localhost|127\\.0\\.0|^\\d{4,5}$/]",
          message: 'FORBIDDEN: Dangerous fallback. Throw error if missing instead.',
        },
      ],
    },
  },
  {
    ignores: ['node_modules/**', 'dist/**', 'coverage/**', 'cluster-hooks/**', 'hooks/**'],
  },
  prettierConfig,
];
