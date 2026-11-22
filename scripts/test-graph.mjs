import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';
import { buildGraphIndex, createWheelGesture, layoutGraph, scopeGraph, stepGraphRange } from '../src/graph.ts';

const seed = JSON.parse(readFileSync(new URL('./fixtures/graph-seed.json', import.meta.url), 'utf8'));
const memory = (id, repositoryId) => ({
  id, repositoryId, title: id, summary: id, kind: '技术决策', tags: [],
  updatedAt: '2026-11-21T00:00:00Z', path: `${repositoryId}/${id}.md`,
  embeddingStatus: 'ready', embeddingError: null,
});
const graph = buildGraphIndex({
  nodes: seed.nodes.map(node => ({ ...memory(node.id, node.repo), title: node.title })),
  edges: seed.links.map(edge => ({ source: edge.from, target: edge.to })),
});
const options = { repositoryId: null, showIsolated: true, centerId: 'FE-018', range: 0 };

for (const [range, nodeCount, edgeCount] of [[0, 11, 14], [1, 1, 0], [2, 5, 5], [3, 8, 9], [4, 11, 14]]) {
  const scope = scopeGraph(graph, { ...options, range });
  assert.equal(scope.nodes.length, nodeCount, `range ${range} nodes`);
  assert.equal(scope.edges.length, edgeCount, `range ${range} edges`);
  if (range > 0) {
    assert.equal(scope.distances.get('FE-018'), 0);
    assert([...scope.distances.values()].every(distance => distance < range));
  }
  const positions = layoutGraph(scope);
  assert.equal(positions.size, nodeCount);
  assert.deepEqual(layoutGraph(scope), positions);
  assert.deepEqual(layoutGraph({ ...scope, nodes: [...scope.nodes].reverse(), edges: [...scope.edges].reverse(), distances: new Map([...scope.distances].reverse()) }), positions);
  assert([...positions.values()].every(point => Object.values(point).every(Number.isFinite)));
  if (range > 0) {
    assert.equal(Math.hypot(...Object.values(positions.get('FE-018'))), 0);
    for (const [nearId, nearDistance] of scope.distances) {
      for (const [farId, farDistance] of scope.distances) {
        if (nearDistance < farDistance) {
          assert(Math.hypot(...Object.values(positions.get(nearId))) < Math.hypot(...Object.values(positions.get(farId))), 'BFS layers retain radial order');
        }
      }
    }
  }
}
assert.equal(graph.degrees.get('FE-018'), 4);
assert(graph.adjacency.get('API-012').has('FE-018'));

const filtered = buildGraphIndex({
  nodes: [memory('A', 'one'), memory('B', 'one'), memory('I', 'one'), memory('C', 'two'), memory('D', 'two')],
  edges: [{ source: 'A', target: 'B' }, { source: 'B', target: 'C' }, { source: 'C', target: 'D' }],
});
const selection = { repositoryId: 'one', showIsolated: true, centerId: 'A', range: 0 };
assert.deepEqual(scopeGraph(filtered, selection).nodes.map(node => node.id), ['A', 'B', 'I']);
assert.equal(scopeGraph(filtered, selection).edges.length, 1);
assert.deepEqual(scopeGraph(filtered, { ...selection, showIsolated: false }).nodes.map(node => node.id), ['A', 'B']);
assert.deepEqual(scopeGraph(filtered, { ...selection, range: 4 }).nodes.map(node => node.id), ['A', 'B']);
assert.equal(scopeGraph(filtered, { ...selection, range: 4, centerId: 'C' }).nodes.length, 0);
assert.equal(scopeGraph(filtered, { ...selection, range: 1, centerId: 'I' }).nodes.length, 1);
assert.equal(scopeGraph(filtered, { ...selection, range: 1, centerId: 'I', showIsolated: false }).nodes.length, 0);
const across = scopeGraph(filtered, { ...selection, repositoryId: null, range: 4 });
assert.deepEqual([...across.distances], [['A', 0], ['B', 1], ['C', 2], ['D', 3]]);
const shortcut = buildGraphIndex({ nodes: filtered.nodes, edges: [...filtered.edges, { source: 'A', target: 'C' }] });
assert.equal(scopeGraph(shortcut, { ...selection, repositoryId: null, range: 4 }).distances.get('D'), 2);

const hub = buildGraphIndex({
  nodes: ['Z', 'A', 'B', 'C', 'I'].map(id => memory(id, 'one')),
  edges: ['A', 'B', 'C'].map(target => ({ source: 'Z', target })),
});
const hubPositions = layoutGraph(scopeGraph(hub, options));
assert.deepEqual(hubPositions.get('Z'), { x: 0, y: 0, z: 0 }, 'highest-degree real memory anchors the global cloud');
assert([...hubPositions].filter(([id]) => id !== 'Z').every(([, point]) => Math.hypot(...Object.values(point)) > 0));
const tie = buildGraphIndex({
  nodes: ['B', 'Z1', 'Z2', 'A', 'Z3', 'Z4'].map(id => memory(id, 'one')),
  edges: [{ source: 'B', target: 'Z1' }, { source: 'B', target: 'Z2' }, { source: 'A', target: 'Z3' }, { source: 'A', target: 'Z4' }],
});
assert.deepEqual(layoutGraph(scopeGraph(tie, options)).get('A'), { x: 0, y: 0, z: 0 }, 'degree ties use stable memory ID');
assert.equal(layoutGraph(scopeGraph(buildGraphIndex({ nodes: [], edges: [] }), options)).size, 0);

const wheel = (deltaY, deltaMode = 0, deltaX = 0) => ({ deltaX, deltaY, deltaMode });
const trackpad = createWheelGesture();
assert.equal(trackpad.step(wheel(4), 0), 1);
assert.equal(trackpad.step(wheel(19), 16), 0);
assert.equal(trackpad.step(wheel(100), 40), 0);
assert.equal(trackpad.step(wheel(3), 110), 0);
assert.equal(trackpad.step(wheel(-5), 300), -1);
assert.equal(trackpad.step(wheel(-2), 330), 0);
assert.equal(trackpad.step(wheel(0), 340), 0);
assert.equal(trackpad.step(wheel(2, 0, 20), 350), 0);
trackpad.reset();
assert.equal(trackpad.step(wheel(4), 360), 1);
const mouse = createWheelGesture();
assert.equal(mouse.step(wheel(100), 0), 1);
assert.equal(mouse.step(wheel(100), 60), 1);
assert.equal(mouse.step(wheel(-120), 100), -1);
assert.equal(mouse.step(wheel(-3, 1), 120), -1);
assert.equal(stepGraphRange(4, 1), 4);
assert.equal(stepGraphRange(0, -1), 0);
let range = 0;
for (let step = 0; step < 4; step++) range = stepGraphRange(range, 1);
assert.equal(range, 4);
for (let step = 0; step < 4; step++) range = stepGraphRange(range, -1);
assert.equal(range, 0);

const large = buildGraphIndex({
  nodes: Array.from({ length: 10_000 }, (_, index) => memory(`N-${index}`, `repo-${index % 3}`)),
  edges: Array.from({ length: 9_999 }, (_, index) => ({ source: `N-${index}`, target: `N-${index + 1}` })),
});
const started = performance.now();
const largeScope = scopeGraph(large, { ...options, centerId: 'N-5000', range: 4 });
assert.equal(largeScope.nodes.length, 7);
assert.equal(largeScope.edges.length, 6);
const globalScope = scopeGraph(large, options);
const globalPositions = layoutGraph(globalScope);
assert.equal(globalPositions.size, 10_000);
const elapsed = performance.now() - started;
assert.deepEqual(layoutGraph({ ...globalScope, nodes: [...globalScope.nodes].reverse(), edges: [...globalScope.edges].reverse() }), globalPositions);
assert.deepEqual(globalPositions.get('N-1'), { x: 0, y: 0, z: 0 });
assert([...globalPositions.values()].every(point => Object.values(point).every(Number.isFinite) && Math.hypot(...Object.values(point)) <= 1));
for (const repositoryId of ['repo-0', 'repo-1', 'repo-2']) {
  const points = large.nodes.filter(node => node.repositoryId === repositoryId).map(node => globalPositions.get(node.id));
  for (const axis of ['x', 'y', 'z']) {
    assert(points.some(point => point[axis] < -.3) && points.some(point => point[axis] > .3), `${repositoryId} spans both sides of ${axis}, rather than a flat or separate island`);
  }
}
const allLocal = scopeGraph(hub, { ...options, centerId: 'Z', range: 2 });
assert.deepEqual(layoutGraph(allLocal).get('Z'), { x: 0, y: 0, z: 0 });
assert([...layoutGraph(allLocal).values()].some(point => Math.abs(point.z) > .2), 'local neighbors have actual spatial depth');
console.log(`Graph checks passed: spherical volume, real hub anchor, stable order, BFS shells, repository scope, isolated nodes and auxiliary gesture helpers. 10,000-node scope + layout ${elapsed.toFixed(2)} ms.`);
