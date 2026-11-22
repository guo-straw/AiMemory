import { readFile, mkdir, writeFile, access } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const project = dirname(dirname(fileURLToPath(import.meta.url)));
const destination = join(project, 'dev-data', 'demo-vault');
try {
  await access(destination);
  console.log(`示例记忆库已存在，可在应用中选择此目录\n${destination}`);
  process.exit(0);
} catch (error) {
  if (error.code !== 'ENOENT') throw error;
}

const seed = JSON.parse(await readFile(join(project, 'design', 'source', 'graph-seed.json'), 'utf8'));
const kinds = { 'FE-009': '问题排查', 'FE-014': '实现约束', 'API-012': '实现约束' };
for (const node of seed.nodes) {
  const directory = join(destination, node.repo);
  await mkdir(directory, { recursive: true });
  const related = seed.links.flatMap(link => link.from === node.id ? [link.to] : link.to === node.id ? [link.from] : []);
  const fields = {
    id: node.id,
    repository_name: node.repo,
    repository_remote: `https://github.com/aimemory-demo/${node.repo}.git`,
    title: node.title,
    kind: kinds[node.id] ?? '技术决策',
    tags: ['设计示例', '异步导出'],
    related_ids: related,
    source: node.id === 'FE-018' ? 'Codex · 异步导出进度 · 轮次 18（设计示例）' : '知迹 KnowTrace 设计示例',
    created_at: '2026-11-21T00:00:00Z',
    updated_at: '2026-11-21T00:00:00Z',
  };
  let markdown = node.markdown;
  if (node.id === 'FE-018') {
    markdown = '# 任务进度改用短轮询\n\n## 结论\n异步导出改为每 2 秒查询一次任务状态。任务进入终态或用户离开页面时，停止轮询。\n\n## 详细说明\n旧版代理会缓冲 SSE 响应，导出页无法持续收到进度事件。前端改为查询任务状态接口，收到 `succeeded` 或 `failed` 后停止，离开页面时清理计时器。\n\n## 原文依据\n> 旧代理会缓冲 SSE。前端先改成 2 秒轮询，到了终态就停，离开页面也要清掉计时器。\n';
  }
  const frontmatter = Object.entries(fields).map(([key, value]) => `${key}: ${JSON.stringify(value)}`).join('\n');
  await writeFile(join(directory, `${node.id}.md`), `---\n${frontmatter}\n---\n\n${markdown.trim()}\n`, 'utf8');
}
console.log(`已生成 ${seed.nodes.length} 条设计示例。请在应用的本地记忆库设置中选择\n${destination}`);
