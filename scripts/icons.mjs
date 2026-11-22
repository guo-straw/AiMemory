import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const icons = join(root, 'src-tauri', 'icons');
const output = join(root, 'dev-data', 'icon-build');
const cli = join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
const source = join(icons, 'source.png');
mkdirSync(output, { recursive: true });

const result = spawnSync(process.execPath, [cli, 'icon', source, '--output', output], {
  cwd: root,
  stdio: 'inherit',
});
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);

for (const file of ['32x32.png', '64x64.png', '128x128.png', '128x128@2x.png', 'icon.png', 'icon.icns', 'icon.ico']) {
  copyFileSync(join(output, file), join(icons, file));
}
console.log('已从 source.png 生成 macOS、Windows 与 PNG 图标。');
