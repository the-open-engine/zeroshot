/**
 * TemplateResolver - Resolves parameterized cluster templates
 *
 * Takes a base template with {{param}} placeholders and resolves them
 * with provided parameter values. Pure data transformation, no magic.
 *
 * Resolution rules:
 * 1. Load base template JSON
 * 2. Deep clone
 * 3. Walk all values, replace {{param}} with params[param]
 * 4. Handle conditional agents via "condition" field
 * 5. Fail hard if any {{param}} remains unresolved
 */

const fs = require('fs');
const path = require('path');

class TemplateResolver {
  /**
   * @param {string} templatesDir
   */
  constructor(templatesDir) {
    this.templatesDir = templatesDir;
    this.baseTemplatesDir = path.join(templatesDir, 'base-templates');
  }

  /**
   * Resolve a template with parameters
   * @param {string} baseName - Name of base template (without .json)
   * @param {Object} params - Parameter values to substitute
   * @returns {Object} Resolved cluster config
   */
  resolve(baseName, params) {
    // Load base template
    const templatePath = path.join(this.baseTemplatesDir, `${baseName}.json`);
    if (!fs.existsSync(templatePath)) {
      throw new Error(`Base template not found: ${baseName} (looked in ${templatePath})`);
    }

    const templateJson = fs.readFileSync(templatePath, 'utf8');
    const template = JSON.parse(templateJson);

    // Validate required params
    this._validateParams(template, params);

    // Apply defaults for missing params (e.g., timeout: 0)
    const paramsWithDefaults = this._applyDefaults(template, params);

    // Deep clone and resolve
    const resolved = this._resolveObject(JSON.parse(JSON.stringify(template)), paramsWithDefaults);

    // Filter out conditional agents that don't meet their condition
    if (resolved.agents) {
      resolved.agents = resolved.agents.filter((/** @type {any} */ agent) => {
        if (!agent.condition) return true;
        const conditionMet = this._evaluateCondition(agent.condition, params);
        delete agent.condition; // Remove condition field from final output
        return conditionMet;
      });
    }

    // Verify no unresolved placeholders remain
    this._verifyResolved(resolved);

    // Remove params schema from output (it's metadata, not config)
    delete resolved.params;

    return resolved;
  }

  /**
   * Validate that required params are provided
   * @private
   * @param {any} template
   * @param {any} params
   */
  _validateParams(template, params) {
    if (!template.params) return;

    const missing = [];
    for (const [name, schema] of Object.entries(template.params)) {
      if (params[name] === undefined && schema.default === undefined) {
        missing.push(name);
      }
    }

    if (missing.length > 0) {
      throw new Error(`Missing required params: ${missing.join(', ')}`);
    }
  }

  /**
   * Apply template defaults for any missing params
   * @private
   * @param {any} template
   * @param {any} params
   * @returns {any} params with defaults applied
   */
  _applyDefaults(template, params) {
    if (!template.params) return params;

    const result = { ...params };
    for (const [name, schema] of Object.entries(template.params)) {
      if (result[name] === undefined && schema.default !== undefined) {
        result[name] = schema.default;
      }
    }
    return result;
  }

  /**
   * Recursively resolve placeholders in an object
   * @private
   * @param {any} obj
   * @param {any} params
   * @returns {any}
   */
  _resolveObject(obj, params) {
    if (obj === null || obj === undefined) {
      return obj;
    }

    if (typeof obj === 'string') {
      return this._resolveString(obj, params);
    }

    if (Array.isArray(obj)) {
      return obj.map((item) => this._resolveObject(item, params));
    }

    if (typeof obj === 'object') {
      /** @type {any} */
      const result = {};
      for (const [key, value] of Object.entries(obj)) {
        result[key] = this._resolveObject(value, params);
      }
      return result;
    }

    return obj;
  }

  /**
   * Resolve placeholders in a string
   * Supports: {{param}} and {{#if condition}}...{{/if}}
   * @private
   * @param {any} str
   * @param {any} params
   * @returns {any}
   */
  _resolveString(str, params) {
    // Handle simple {{param}} substitutions
    let result = str.replace(
      /\{\{(\w+)\}\}/g,
      (/** @type {any} */ _match, /** @type {any} */ paramName) => {
        if (params[paramName] !== undefined) {
          return params[paramName];
        }
        // Return match unchanged if param not found (will be caught by verify)
        return _match;
      }
    );

    // Handle {{#if condition}}...{{/if}} blocks
    result = result.replace(
      /\{\{#if\s+([^}]+)\}\}([\s\S]*?)\{\{\/if\}\}/g,
      (/** @type {any} */ _match, /** @type {any} */ condition, /** @type {any} */ content) => {
        const conditionMet = this._evaluateCondition(condition, params);
        return conditionMet ? content : '';
      }
    );

    // Clean up multiple newlines from removed conditionals
    result = result.replace(/\n{3,}/g, '\n\n');

    return result;
  }

  /**
   * Evaluate a simple condition expression
   * Supports: param == 'value', param != 'value', {{param}} >= N
   * @private
   * @param {any} condition
   * @param {any} params
   * @returns {boolean}
   */
  _evaluateCondition(condition, params) {
    // Replace {{param}} with actual values first
    let expr = condition.trim();

    // Replace {{param}} placeholders
    expr = expr.replace(
      /\{\{(\w+)\}\}/g,
      (/** @type {any} */ _match, /** @type {any} */ paramName) => {
        const value = params[paramName];
        if (value === undefined) return 'undefined';
        if (typeof value === 'string') return `"${value}"`;
        return String(value);
      }
    );

    // Replace bare param names
    for (const [name, value] of Object.entries(params)) {
      const regex = new RegExp(`\\b${name}\\b`, 'g');
      if (typeof value === 'string') {
        expr = expr.replace(regex, `"${value}"`);
      } else {
        expr = expr.replace(regex, String(value));
      }
    }

    try {
      // Parse and evaluate simple comparison expressions without eval
      return this._evaluateSimpleExpression(expr);
    } catch {
      console.error(`Failed to evaluate condition: ${condition} (resolved: ${expr})`);
      return false;
    }
  }

  /**
   * Evaluate simple comparison expressions without eval
   * Supports: ==, !=, <, >, <=, >=, &&, ||
   * @private
   * @param {string} expr
   * @returns {boolean}
   */
  _evaluateSimpleExpression(expr) {
    // Handle logical operators (&&, ||) by splitting and recursing
    if (expr.includes('||')) {
      const parts = expr.split('||');
      return parts.some((part) => this._evaluateSimpleExpression(part.trim()));
    }
    if (expr.includes('&&')) {
      const parts = expr.split('&&');
      return parts.every((part) => this._evaluateSimpleExpression(part.trim()));
    }

    // Handle comparison operators
    const comparisonOps = ['==', '!=', '<=', '>=', '<', '>'];
    for (const op of comparisonOps) {
      if (expr.includes(op)) {
        const [left, right] = expr.split(op).map((s) => s.trim());
        const leftVal = this._parseValue(left);
        const rightVal = this._parseValue(right);

        switch (op) {
          case '==':
            return leftVal === rightVal;
          case '!=':
            return leftVal !== rightVal;
          case '<':
            return leftVal < rightVal;
          case '>':
            return leftVal > rightVal;
          case '<=':
            return leftVal <= rightVal;
          case '>=':
            return leftVal >= rightVal;
        }
      }
    }

    // If no operator found, treat as boolean literal or truthy value
    return this._parseValue(expr) ? true : false;
  }

  /**
   * Parse a value from string (number, boolean, string literal)
   * @private
   * @param {string} str
   * @returns {any}
   */
  _parseValue(str) {
    str = str.trim();

    // String literals (single or double quotes)
    if ((str.startsWith('"') && str.endsWith('"')) || (str.startsWith("'") && str.endsWith("'"))) {
      return str.slice(1, -1);
    }

    // Boolean literals
    if (str === 'true') return true;
    if (str === 'false') return false;
    if (str === 'undefined' || str === 'null') return undefined;

    // Numbers
    if (/^-?\d+(\.\d+)?$/.test(str)) {
      return parseFloat(str);
    }

    // Return as-is for other cases
    return str;
  }

  /**
   * Verify no unresolved placeholders remain
   * @private
   * @param {any} obj
   */
  _verifyResolved(obj) {
    const unresolved = this._findUnresolved(obj);
    if (unresolved.length > 0) {
      throw new Error(`Unresolved template placeholders: ${unresolved.join(', ')}`);
    }
  }

  /**
   * Find all unresolved {{param}} placeholders
   * @private
   * @param {any} obj
   * @param {string} pathPrefix
   * @returns {string[]}
   */
  _findUnresolved(obj, pathPrefix = '') {
    const unresolved = [];

    if (typeof obj === 'string') {
      const matches = obj.match(/\{\{(\w+)\}\}/g);
      if (matches) {
        unresolved.push(...matches.map((m) => `${pathPrefix}: ${m}`));
      }
    } else if (Array.isArray(obj)) {
      obj.forEach((item, i) => {
        unresolved.push(...this._findUnresolved(item, `${pathPrefix}[${i}]`));
      });
    } else if (obj && typeof obj === 'object') {
      for (const [key, value] of Object.entries(obj)) {
        unresolved.push(...this._findUnresolved(value, `${pathPrefix}.${key}`));
      }
    }

    return unresolved;
  }

  /**
   * List available base templates
   * @returns {string[]}
   */
  listTemplates() {
    if (!fs.existsSync(this.baseTemplatesDir)) {
      return [];
    }
    return fs
      .readdirSync(this.baseTemplatesDir)
      .filter((f) => f.endsWith('.json'))
      .map((f) => f.replace('.json', ''));
  }

  /**
   * Get template metadata (name, description, params)
   * @param {any} baseName
   * @returns {any}
   */
  getTemplateInfo(baseName) {
    const templatePath = path.join(this.baseTemplatesDir, `${baseName}.json`);
    if (!fs.existsSync(templatePath)) {
      return null;
    }
    const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
    return {
      name: template.name,
      description: template.description,
      params: template.params || {},
    };
  }
}

module.exports = TemplateResolver;
