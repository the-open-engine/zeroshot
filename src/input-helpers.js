/**
 * Input Helpers - Create input data from text or files
 *
 * Provides fallback input methods for non-issue-based input:
 * - Plain text input
 * - File input (markdown)
 */

const fs = require('fs');
const path = require('path');

class InputHelpers {
  /**
   * Create a plain text input wrapper
   * @param {String} text - Plain text input
   * @returns {Object} Structured context
   */
  static createTextInput(text) {
    return {
      number: null,
      title: 'Manual Input',
      body: text,
      labels: [],
      comments: [],
      url: null,
      context: `# Manual Input\n\n${text}\n`,
    };
  }

  /**
   * Create input from markdown file
   * @param {String} filePath - Path to markdown file (.md or .markdown)
   * @returns {Object} Structured context matching issue format
   */
  static createFileInput(filePath) {
    // Resolve relative paths
    const resolvedPath = path.resolve(filePath);

    // Validate file exists
    if (!fs.existsSync(resolvedPath)) {
      throw new Error(`File not found: ${filePath}`);
    }

    // Read file content
    const fileContent = fs.readFileSync(resolvedPath, 'utf8');

    // Extract title from first header or use filename
    const headerMatch = fileContent.match(/^#\s+(.+)$/m);
    const extractedTitle = headerMatch ? headerMatch[1].trim() : null;
    const fallbackTitle = path.basename(filePath, path.extname(filePath));
    const title = extractedTitle || fallbackTitle;

    return {
      number: null,
      title,
      body: fileContent,
      labels: [],
      comments: [],
      url: null,
      context: `# ${title}\n\n${fileContent}\n`,
    };
  }
}

module.exports = InputHelpers;
