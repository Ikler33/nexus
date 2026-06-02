// Генерирует placeholder app-иконку 1024×1024 (PNG, RGBA) без внешних зависимостей —
// только встроенный zlib. Источник для `cargo tauri icon`, который собирает полный
// набор платформенных иконок. Заменить на брендовую иконку перед релизом.
//
// Использование: node scripts/gen-icon.mjs [output.png]
import { writeFileSync } from 'node:fs';
import { deflateSync } from 'node:zlib';

const W = 1024;
const H = 1024;
const BG = [79, 70, 229]; // indigo (--color-accent)
const INK = [245, 245, 250]; // почти-белый

// Глиф «N»: две вертикальные стойки + диагональ.
function isInk(x, y) {
  const m = 256;
  const sz = 512; // бокс глифа [256, 768]
  const bar = 96;
  const x0 = m;
  const x1 = m + sz;
  const y0 = m;
  const y1 = m + sz;
  if (y < y0 || y > y1) return false;
  if (x >= x0 && x < x0 + bar) return true; // левая стойка
  if (x > x1 - bar && x <= x1) return true; // правая стойка
  const t = (y - y0) / (y1 - y0);
  const cx = x0 + t * (sz - bar); // диагональ TL→BR
  return x >= cx && x < cx + bar;
}

function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return (~c) >>> 0;
}

function chunk(type, data) {
  const typeBuf = Buffer.from(type, 'ascii');
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])));
  return Buffer.concat([len, typeBuf, data, crc]);
}

const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(W, 0);
ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA

const rowLen = 1 + W * 4;
const raw = Buffer.alloc(rowLen * H);
for (let y = 0; y < H; y++) {
  const off = y * rowLen;
  raw[off] = 0; // filter: none
  for (let x = 0; x < W; x++) {
    const p = off + 1 + x * 4;
    const [r, g, b] = isInk(x, y) ? INK : BG;
    raw[p] = r;
    raw[p + 1] = g;
    raw[p + 2] = b;
    raw[p + 3] = 255;
  }
}

const png = Buffer.concat([
  sig,
  chunk('IHDR', ihdr),
  chunk('IDAT', deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
]);

const out = process.argv[2] || 'app-icon.png';
writeFileSync(out, png);
console.log(`wrote ${out} (${png.length} bytes)`);
