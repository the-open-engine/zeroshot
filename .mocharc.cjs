const hasCliTestFile = process.argv
  .slice(2)
  .some((arg) => typeof arg === 'string' && /\.test\.[jt]s$/.test(arg));

const config = {
  parallel: true,
  jobs: 4,
  timeout: 10000,
  slow: 1000,
};

if (!hasCliTestFile) {
  config.spec = 'tests/**/*.test.js';
}

module.exports = config;
