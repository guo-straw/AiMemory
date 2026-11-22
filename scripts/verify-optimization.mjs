import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const output = join(root, 'dev-data', 'optimization-acceptance');
await mkdir(output, { recursive: true });
const env = { ...process.env, PATH: `${dirname(process.execPath)}:${process.env.PATH}` };
const results = [];
for (const [name, command, args] of [
  ['Rust', process.execPath, ['scripts/desktop.mjs', 'test']],
  ['Graph', 'pnpm', ['test:graph']],
  ['Model services', 'pnpm', ['test:model-services']],
  ['Archive sidebar', 'pnpm', ['test:archive-sidebar']],
  ['Frontend build', 'pnpm', ['build']],
  ['Diff whitespace', 'git', ['diff', '--check']],
]) {
  const started = performance.now();
  const result = await new Promise(resolve => {
    const child = spawn(command, args, { cwd: root, env });
    let text = '';
    child.stdout.on('data', data => { text += data; });
    child.stderr.on('data', data => { text += data; });
    child.on('error', error => resolve({ code: 1, text: error.message }));
    child.on('close', code => resolve({ code, text }));
  });
  const passed = [...result.text.matchAll(/test result: ok\. (\d+) passed/g)].reduce((sum, match) => sum + Number(match[1]), 0);
  const entry = { name, exitCode: result.code, elapsedMs: performance.now() - started, ...(name === 'Rust' ? { passed } : {}), log: result.text };
  results.push(entry);
  await writeFile(join(output, 'checks.json'), JSON.stringify({ generatedAt: new Date().toISOString(), results }, null, 2));
  console.log(`${name}: ${result.code === 0 ? 'passed' : 'FAILED'}${name === 'Rust' ? ` (${passed} tests)` : ''}`);
  if (result.code !== 0) {
    console.error(result.text.slice(-6000));
    process.exitCode = 1;
    break;
  }
}
