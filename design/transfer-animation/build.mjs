import { readFile, writeFile } from 'node:fs/promises';
const read = (path) => readFile(new URL(path, import.meta.url));
const [template, style, engine, preview, sprite, reference] = await Promise.all([
  read('index.template.html'), read('style.css'), read('transfer-indicator.mjs'), read('preview.js'),
  read('assets/transfer-icons.png'), read('assets/approved-reference.png'),
]);
const source = `${engine.toString().replace('export class ', 'class ')}\n${preview}`;
const html = template.toString()
  .replace('/* INLINE_STYLES */', () => style.toString())
  .replace('/* INLINE_SCRIPT */', () => source)
  .replace('__SPRITE_DATA_URI__', `data:image/png;base64,${sprite.toString('base64')}`)
  .replace('__REFERENCE_DATA_URI__', `data:image/png;base64,${reference.toString('base64')}`);
await writeFile(new URL('index.html', import.meta.url), html);
console.log('Built self-contained index.html (no network or dependencies required).');
