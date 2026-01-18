import js from '@eslint/js';
import unusedImports from 'eslint-plugin-unused-imports';
import securityPlugin from 'eslint-plugin-security';
import sonarPlugin from 'eslint-plugin-sonarjs';
import prettierConfig from 'eslint-config-prettier';

export default [
  {
    linterOptions: {
      noInlineConfig: true, // Disallow eslint-disable - FIX CODE, don't disable rules
    },
  },
  js.configs.recommended,
  {
    plugins: {
      'unused-imports': unusedImports,
      security: securityPlugin,
      sonarjs: sonarPlugin,
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
      // SonarJS recommended rules (override errors → warnings for gradual adoption)
      ...sonarPlugin.configs.recommended.rules,

      // Code quality - WARN not ERROR (too many violations, fix gradually)
      'sonarjs/cognitive-complexity': 'warn',
      'sonarjs/no-nested-conditional': 'warn',
      'sonarjs/no-nested-functions': 'warn',
      'sonarjs/no-invariant-returns': 'warn',
      'sonarjs/no-all-duplicated-branches': 'warn',
      'sonarjs/no-duplicated-branches': 'warn',
      'sonarjs/single-character-alternation': 'warn',
      'sonarjs/no-collection-size-mischeck': 'warn',
      'sonarjs/concise-regex': 'off', // Style preference
      'sonarjs/public-static-readonly': 'off', // JS doesn't have readonly modifier
      'sonarjs/single-char-in-character-classes': 'warn', // Style preference
      // Disable sonarjs/no-unused-vars - we use unused-imports plugin with varsIgnorePattern: '^_'
      'sonarjs/no-unused-vars': 'off',

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
      // WARN for now - many violations exist (fix gradually)
      'no-param-reassign': 'warn',

      // Code quality
      complexity: ['warn', 20],
      'max-depth': ['warn', 4],
      'max-nested-callbacks': ['warn', 4],
      'max-params': ['warn', 5],
      'max-lines-per-function': ['warn', { max: 150, skipBlankLines: true, skipComments: true }],
      'max-lines': ['warn', { max: 500, skipBlankLines: true, skipComments: true }],
      'sonarjs/no-identical-functions': 'error',

      // Security - eslint-plugin-security
      // NOTE: detect-child-process off because we have safe-exec wrapper enforcement via no-restricted-syntax
      'security/detect-child-process': 'off',
      'security/detect-eval-with-expression': 'error',
      // NOTE: detect-object-injection off - high false positive for CLI tools with controlled object access
      'security/detect-object-injection': 'off',
      // NOTE: warn not error - false positives on anchored patterns with controlled input
      'security/detect-unsafe-regex': 'warn',
      'security/detect-non-literal-regexp': 'warn',
      // NOTE: detect-non-literal-fs-filename off - CLI tools legitimately use variable paths
      'security/detect-non-literal-fs-filename': 'off',
      'security/detect-non-literal-require': 'error',
      'security/detect-buffer-noassert': 'error',
      'security/detect-new-buffer': 'error',
      'security/detect-pseudoRandomBytes': 'error',
      'security/detect-possible-timing-attacks': 'warn',
      'security/detect-bidi-characters': 'error',

      // Disable pedantic sonarjs rules (high false positive rate for CLI/orchestrator)
      'sonarjs/todo-tag': 'off',
      'sonarjs/deprecation': 'off',
      'sonarjs/constructor-for-side-effects': 'off',
      'sonarjs/publicly-writable-directories': 'off',
      'sonarjs/file-permissions': 'off',
      'sonarjs/arguments-order': 'off',
      'sonarjs/slow-regex': 'off',
      'sonarjs/no-clear-text-protocols': 'off',
      'sonarjs/prefer-immediate-return': 'off',
      'sonarjs/prefer-single-boolean-return': 'off',
      'sonarjs/no-nested-template-literals': 'off',
      'sonarjs/no-commented-code': 'off',
      'sonarjs/no-gratuitous-expressions': 'off',
      // NOTE: os-command and no-os-command-from-path off - zeroshot IS an orchestrator that spawns commands
      'sonarjs/os-command': 'off',
      'sonarjs/no-os-command-from-path': 'off',

      // Basic security
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
    rules: {
      // Tests have nested describe/it blocks - allow deeper nesting
      'sonarjs/no-nested-functions': 'off',
      // Test files can be large (many test cases)
      'max-lines': 'off',
      // Allow unused vars in tests (setup fixtures)
      'sonarjs/no-unused-vars': 'off',
      // Allow unused collections in tests (assertion helpers)
      'sonarjs/no-unused-collection': 'off',
      // Math.random() fine in tests
      'sonarjs/pseudo-random': 'off',
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
    // TUI/CLI/streaming files use ANSI escape codes for terminal colors - allow control characters
    files: [
      'src/tui/**/*.js',
      'src/streaming/*.js',
      'src/streaming/**/*.js',
      'src/status-footer.js',
      'task-lib/tui.js',
      'task-lib/tui/**/*.js',
      'cli/**/*.js',
    ],
    rules: {
      'no-control-regex': 'off',
      'sonarjs/no-control-regex': 'off',
      // Unused vars common in streaming (partial destructuring)
      'sonarjs/no-unused-vars': 'off',
    },
  },
  {
    // Large files that need refactoring - temporary overrides
    // TODO: Split these files into smaller modules
    files: ['cli/index.js', 'src/agent/agent-task-executor.js', 'src/agent/agent-lifecycle.js'],
    rules: {
      'max-lines': 'off',
    },
  },
  {
    // Math.random() used for non-security purposes (jitter, distribution)
    files: ['src/**/*.js', 'lib/**/*.js', 'task-lib/**/*.js'],
    rules: {
      'sonarjs/pseudo-random': 'warn',
    },
  },
  {
    // Message formatters always return true (handled) - valid handler pattern
    files: ['cli/message-formatters-*.js'],
    rules: {
      'sonarjs/no-invariant-returns': 'off',
    },
  },
  {
    ignores: ['node_modules/**', 'dist/**', 'coverage/**', 'cluster-hooks/**', 'hooks/**'],
  },
  prettierConfig,
];
