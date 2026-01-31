const path = require('path');
const { pathToFileURL } = require('url');

const PROJECT_ROOT = path.resolve(__dirname, '..', '..');

const loadTuiModule = (relativePath) => {
  const fullPath = path.resolve(PROJECT_ROOT, relativePath);
  return import(pathToFileURL(fullPath).href);
};

module.exports = { loadTuiModule };
