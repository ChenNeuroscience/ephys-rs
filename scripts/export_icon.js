// Exports from assets/icon.svg: icon.png (the window icon, embedded in the binary), icon-1024.png (for the macOS app
// bundle's icon, made when packaging) and icon.ico (the Windows executable's icon, embedded by example/build.rs).
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

// An .ico is a list of images: here PNGs, at the sizes Windows asks for.
const pngs = [16, 24, 32, 48, 64, 128, 256].map((width) => ({
    width,
    png: new Resvg(svg, { fitTo: { mode: 'width', value: width } }).render().asPng(),
}));
const header = Buffer.alloc(6 + 16 * pngs.length);
header.writeUInt16LE(1, 2); // an icon
header.writeUInt16LE(pngs.length, 4);
let offset = header.length;
pngs.forEach(({ width, png }, i) => {
    const entry = 6 + 16 * i;
    header.writeUInt8(width % 256, entry); // 256 is written as 0
    header.writeUInt8(width % 256, entry + 1);
    header.writeUInt16LE(1, entry + 4); // colour planes
    header.writeUInt16LE(32, entry + 6); // bits a pixel
    header.writeUInt32LE(png.length, entry + 8);
    header.writeUInt32LE(offset, entry + 12);
    offset += png.length;
});
fs.writeFileSync(path.join(assets, 'icon.ico'), Buffer.concat([header, ...pngs.map((p) => p.png)]));
console.log('Exported assets/icon.ico from assets/icon.svg');
