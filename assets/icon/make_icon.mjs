/** Export light.svg (the logo for dark backgrounds) and light-dark.svg (for light ones) to transparent PNGs and Windows
 * ICOs. Requires sharp. `node make_icon.mjs light-dark` exports one of them; no argument exports both. */
import { createRequire } from 'node:module';
import { readFile, writeFile } from 'node:fs/promises';
const sharp = createRequire(import.meta.url)('sharp');
const variants = process.argv.slice(2).length ? process.argv.slice(2) : ['light', 'light-dark'];
for (const name of variants) {
  const source = await readFile(new URL(`./${name}.svg`, import.meta.url));
  const square = await sharp(source, { density: 288 }).trim()
    .resize(840, 840, { fit: 'contain', background: { r: 0, g: 0, b: 0, alpha: 0 } })
    .extend({ top: 92, bottom: 92, left: 92, right: 92, background: { r: 0, g: 0, b: 0, alpha: 0 } })
    .png().toBuffer();
  const frames = new Map();
  for (const size of [16, 24, 32, 48, 64, 96, 128, 256, 512, 1024]) {
    const png = await sharp(square).resize(size, size).png().toBuffer();
    frames.set(size, png);
    await writeFile(new URL(`./${name}-${size}.png`, import.meta.url), png);
  }
  const icoSizes = [16, 24, 32, 48, 64, 128, 256];
  const header = Buffer.alloc(6 + icoSizes.length * 16);
  header.writeUInt16LE(1, 2); header.writeUInt16LE(icoSizes.length, 4);
  let offset = header.length;
  for (const [index, size] of icoSizes.entries()) {
    const position = 6 + index * 16;
    header[position] = header[position + 1] = size === 256 ? 0 : size;
    header.writeUInt16LE(1, position + 4); header.writeUInt16LE(32, position + 6);
    header.writeUInt32LE(frames.get(size).length, position + 8);
    header.writeUInt32LE(offset, position + 12); offset += frames.get(size).length;
  }
  await writeFile(new URL(`./${name}.ico`, import.meta.url), Buffer.concat([header, ...icoSizes.map(size => frames.get(size))]));
  console.log(`${name}: PNG icons and ${name}.ico exported from ${name}.svg`);
}
