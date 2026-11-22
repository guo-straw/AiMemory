import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const cargoHome = join(root, '.tools', 'cargo');
const rustupHome = join(root, '.tools', 'rustup');
const localRust = existsSync(join(cargoHome, 'bin', 'cargo'));
const env = { ...process.env };
env.CARGO_TARGET_DIR = join(root, 'src-tauri', 'target');
if (localRust) {
  env.CARGO_HOME = cargoHome;
  env.RUSTUP_HOME = rustupHome;
  env.PATH = `${join(cargoHome, 'bin')}${process.platform === 'win32' ? ';' : ':'}${env.PATH}`;
}

const args = process.argv.slice(2);
const testing = args[0] === 'test';
const command = testing ? 'cargo' : join(root, 'node_modules', '.bin', process.platform === 'win32' ? 'tauri.cmd' : 'tauri');
const commandArgs = testing ? ['test', '--manifest-path', 'src-tauri/Cargo.toml', ...args.slice(1)] : args.length ? args : ['dev'];
const child = spawn(command, commandArgs, { cwd: root, env, stdio: 'inherit', shell: process.platform === 'win32' });
child.on('error', error => {
  console.error(`启动失败，${error.message}`);
  process.exitCode = 1;
});
child.on('exit', code => { process.exitCode = code ?? 1; });
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => child.kill(signal));
