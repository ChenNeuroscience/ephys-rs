// Exports github.png from assets/github.svg, and from assets/icon.svg: icon.png (the window icon, embedded in the binary), icon-1024.png (for the macOS app
// bundle's icon, made when packaging) and icon.ico (the Windows executable's icon, embedded by examples/chen-neuro/build.rs).
// Development only; needs @resvg/resvg-js on NODE_PATH:
//     NODE_PATH=path/to/node_modules node scripts/export_icon.js
const fs = require('node:fs');
const path = require('node:path');
const { Resvg } = require('@resvg/resvg-js');

const assets = path.join(__dirname, '..', 'assets');
const svg = fs.readFileSync(path.join(assets, 'icon.svg'));
for (const [name, width] of [['icon.png', 256], ['icon-1024.png', 1024]]) {
    const png = new Resvg(svg, { fitTo: { mode: 'width', value: width } }).render().asPng();
    fs.writeFileSync(path.join(assets, name), png);
    console.log(`Exported assets/${name} from assets/icon.svg`);
}

// The mark on the About window's link, white so it can be tinted to suit the theme.
const mark = new Resvg(fs.readFileSync(path.join(assets, 'github.svg')), { fitTo: { mode: 'width', value: 64 } });
fs.writeFileSync(path.join(assets, 'github.png'), mark.render().asPng());
console.log('Exported assets/github.png from assets/github.svg');

// An .ico is a list of images, at the sizes Windows asks for. Windows only reliably reads PNGs at 256, so the smaller
// ones are bitmaps: a header, then the rows bottom up as blue, green, red and alpha, then a mask, unused with alpha.
function bitmap(width) {
    const { pixels } = new Resvg(svg, { fitTo: { mode: 'width', value: width } }).render();
    const header = Buffer.alloc(40);
    header.writeUInt32LE(40, 0);
    header.writeInt32LE(width, 4);
    header.writeInt32LE(width * 2, 8); // the height of the image and the mask together
    header.writeUInt16LE(1, 12); // colour planes
    header.writeUInt16LE(32, 14); // bits a pixel
    const rows = Buffer.alloc(width * width * 4);
    for (let y = 0; y < width; y++) {
        for (let x = 0; x < width; x++) {
            const from = ((width - 1 - y) * width + x) * 4;
            const to = (y * width + x) * 4;
            // Rendered with the colours multiplied by the alpha, which isn't how an icon has them.
            const alpha = pixels[from + 3];
            const colour = (value) => (alpha ? Math.min(255, Math.round((value * 255) / alpha)) : 0);
            [rows[to], rows[to + 1], rows[to + 2], rows[to + 3]] = [colour(pixels[from + 2]), colour(pixels[from + 1]), colour(pixels[from]), alpha];
        }
    }
    const mask = Buffer.alloc(Math.ceil(width / 32) * 4 * width);
    return Buffer.concat([header, rows, mask]);
}
const images = [16, 24, 32, 48, 64, 128].map((width) => ({ width, data: bitmap(width) }));
images.push({ width: 256, data: new Resvg(svg, { fitTo: { mode: 'width', value: 256 } }).render().asPng() });
const header = Buffer.alloc(6 + 16 * images.length);
header.writeUInt16LE(1, 2); // an icon
header.writeUInt16LE(images.length, 4);
let offset = header.length;
images.forEach(({ width, data }, i) => {
    const entry = 6 + 16 * i;
    header.writeUInt8(width % 256, entry); // 256 is written as 0
    header.writeUInt8(width % 256, entry + 1);
    header.writeUInt16LE(1, entry + 4); // colour planes
    header.writeUInt16LE(32, entry + 6); // bits a pixel
    header.writeUInt32LE(data.length, entry + 8);
    header.writeUInt32LE(offset, entry + 12);
    offset += data.length;
});
fs.writeFileSync(path.join(assets, 'icon.ico'), Buffer.concat([header, ...images.map((image) => image.data)]));
console.log('Exported assets/icon.ico from assets/icon.svg');
