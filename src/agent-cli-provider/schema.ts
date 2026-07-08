import { randomUUID } from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

import { parseJson, stringifyJson } from './json';

type SchemaObject = Record<string, unknown> | readonly unknown[];

function isSchemaObject(schema: unknown): schema is SchemaObject {
  return typeof schema === 'object' && schema !== null;
}

function shallowCopySchemaObject(schema: SchemaObject): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(schema)) {
    result[key] = value;
  }
  return result;
}

function objectKeys(schema: unknown): string[] {
  return isSchemaObject(schema) ? Object.keys(schema) : [];
}

function enforceSchemaNode(schema: unknown): unknown {
  if (!isSchemaObject(schema)) {
    return schema;
  }

  const result = shallowCopySchemaObject(schema);
  enforceObjectSchema(result);
  enforceProperties(result);
  enforceItems(result);
  enforceCompositionKeywords(result);
  enforceAdditionalProperties(result);
  return result;
}

function enforceObjectSchema(result: Record<string, unknown>): void {
  if (result.type === 'object') {
    result.additionalProperties = false;
    if (result.properties) {
      result.required = objectKeys(result.properties);
    }
  }
}

function enforceProperties(result: Record<string, unknown>): void {
  const properties = result.properties;
  if (isSchemaObject(properties)) {
    const strictProperties: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(properties)) {
      strictProperties[key] = enforceSchemaNode(value);
    }
    result.properties = strictProperties;
  }
}

function enforceItems(result: Record<string, unknown>): void {
  if (result.items) {
    result.items = enforceSchemaNode(result.items);
  }
}

function enforceCompositionKeywords(result: Record<string, unknown>): void {
  for (const key of ['anyOf', 'oneOf', 'allOf']) {
    const value = result[key];
    if (Array.isArray(value)) {
      result[key] = value.map((item) => enforceSchemaNode(item));
    }
  }
}

function enforceAdditionalProperties(result: Record<string, unknown>): void {
  const additionalProperties = result.additionalProperties;
  if (additionalProperties && isSchemaObject(additionalProperties)) {
    result.additionalProperties = enforceSchemaNode(additionalProperties);
  }
}

export function enforceOpenAIStrictSchema(schema: unknown): unknown {
  return enforceSchemaNode(schema);
}

export function schemaToPromptString(schema: unknown): string {
  return typeof schema === 'string' ? schema : stringifyJson(schema, 2);
}

export function appendJsonSchemaPrompt(context: string, schema: unknown): string {
  const schemaString = schemaToPromptString(schema);
  return (
    context +
    `\n\n## OUTPUT FORMAT (CRITICAL - REQUIRED)

You MUST respond with a JSON object that exactly matches this schema. NO markdown, NO explanation, NO code blocks. ONLY the raw JSON object.

Schema:
\`\`\`json
${schemaString}
\`\`\`

Your response must be ONLY valid JSON. Start with { and end with }. Nothing else.`
  );
}

export function writeStrictOutputSchemaFile(schema: unknown): string {
  const parsedSchema = typeof schema === 'string' ? parseJson(schema) : schema;
  const strictSchema = enforceOpenAIStrictSchema(parsedSchema);
  const schemaText = stringifyJson(strictSchema, 2);
  const schemaDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-schema-'));
  const schemaFile = path.join(schemaDir, `${randomUUID()}.json`);
  fs.writeFileSync(schemaFile, schemaText, { flag: 'wx', mode: 0o600 });
  return schemaFile;
}
