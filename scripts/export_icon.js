// Exports from assets/icon.svg: icon.png (the window icon, embedded in the binary) and icon-1024.png (for the
// macOS app bundle's icon, made when packaging).
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
