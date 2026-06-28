import { readFile, writeFile } from 'node:fs/promises';

const htmlPath = new globalThis.URL(
  '../dist/client/index.html',
  import.meta.url,
);
const serverEntry = new globalThis.URL(
  '../dist/server/entry-server.js',
  import.meta.url,
);

const template = await readFile(htmlPath, 'utf8');
const { renderShell } = await import(serverEntry.href);
const html = template.replace('<!--ssr-outlet-->', renderShell());

await writeFile(htmlPath, html);
