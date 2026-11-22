import type { MemoryGraph, MemorySummary } from './types';

export type GraphRange = 0 | 1 | 2 | 3 | 4;

export const REPOSITORY_COLORS = ['#3566f0', '#318899', '#7564be', '#bd803d', '#ba5981', '#4b8e68'] as const;

export interface GraphIndex extends MemoryGraph {
  byId: Map<string, MemorySummary>;
  adjacency: Map<string, Set<string>>;
  degrees: Map<string, number>;
}

export interface GraphScope extends MemoryGraph {
  distances: Map<string, number>;
  range: GraphRange;
  centerId: string | null;
}

export interface GraphPoint {
  x: number;
  y: number;
  z: number;
}

export function buildGraphIndex(graph: MemoryGraph): GraphIndex {
  const byId = new Map(graph.nodes.map(node => [node.id, node]));
  const adjacency = new Map(graph.nodes.map(node => [node.id, new Set<string>()]));
  for (const edge of graph.edges) {
    adjacency.get(edge.source)!.add(edge.target);
    adjacency.get(edge.target)!.add(edge.source);
  }
  const degrees = new Map([...adjacency].map(([id, neighbors]) => [id, neighbors.size]));
  return { ...graph, byId, adjacency, degrees };
}

export function scopeGraph(index: GraphIndex, options: {
  repositoryId: string | null;
  showIsolated: boolean;
  centerId: string | null;
  range: GraphRange;
}): GraphScope {
  const candidates = index.nodes.filter(node => !options.repositoryId || node.repositoryId === options.repositoryId);
  const candidateIds = new Set(candidates.map(node => node.id));
  const candidateEdges = index.edges.filter(edge => candidateIds.has(edge.source) && candidateIds.has(edge.target));
  const adjacency = new Map(candidates.map(node => [node.id, [] as string[]]));
  for (const edge of candidateEdges) {
    adjacency.get(edge.source)!.push(edge.target);
    adjacency.get(edge.target)!.push(edge.source);
  }
  const nodes = options.showIsolated ? candidates : candidates.filter(node => adjacency.get(node.id)!.length > 0);
  const distances = new Map<string, number>();
  if (options.range === 0) return { nodes, edges: candidateEdges, distances, range: options.range, centerId: options.centerId };
  if (!options.centerId || !nodes.some(node => node.id === options.centerId)) {
    return { nodes: [], edges: [], distances, range: options.range, centerId: options.centerId };
  }

  distances.set(options.centerId, 0);
  let frontier = [options.centerId];
  for (let distance = 1; distance < options.range && frontier.length; distance++) {
    const next: string[] = [];
    for (const id of frontier) {
      for (const neighbor of adjacency.get(id)!) {
        if (distances.has(neighbor)) continue;
        distances.set(neighbor, distance);
        next.push(neighbor);
      }
    }
    frontier = next;
  }
  return {
    nodes: nodes.filter(node => distances.has(node.id)),
    edges: candidateEdges.filter(edge => distances.has(edge.source) && distances.has(edge.target)),
    distances,
    range: options.range,
    centerId: options.centerId,
  };
}

function hash(value: string): number {
  let result = 2166136261;
  for (let index = 0; index < value.length; index++) {
    result = Math.imul(result ^ value.charCodeAt(index), 16777619);
  }
  result ^= result >>> 16;
  result = Math.imul(result, 0x7feb352d);
  result ^= result >>> 15;
  result = Math.imul(result, 0x846ca68b);
  result ^= result >>> 16;
  return (result >>> 0) / 4294967296;
}

function sphereDirection(id: string): GraphPoint {
  const z = hash(`${id}:height`) * 2 - 1;
  const angle = hash(`${id}:angle`) * Math.PI * 2;
  const ring = Math.sqrt(1 - z * z);
  return { x: ring * Math.cos(angle), y: ring * Math.sin(angle), z };
}

export function layoutGraph(scope: GraphScope): Map<string, GraphPoint> {
  const positions = new Map<string, GraphPoint>();
  const nodes = [...scope.nodes].sort((left, right) => left.id < right.id ? -1 : left.id > right.id ? 1 : 0);
  if (scope.range > 0) {
    for (const node of nodes) {
      const distance = scope.distances.get(node.id)!;
      const direction = sphereDirection(`${scope.centerId}:${distance}:${node.id}`);
      const radius = distance * .94 / Math.max(1, scope.range - 1);
      positions.set(node.id, distance === 0 ? { x: 0, y: 0, z: 0 } : {
        x: direction.x * radius,
        y: direction.y * radius,
        z: direction.z * radius,
      });
    }
    return positions;
  }

  if (!nodes.length) return positions;
  const degrees = new Map(nodes.map(node => [node.id, 0]));
  for (const edge of scope.edges) {
    degrees.set(edge.source, degrees.get(edge.source)! + 1);
    degrees.set(edge.target, degrees.get(edge.target)! + 1);
  }
  let anchor = nodes[0]!;
  for (const node of nodes) {
    if (degrees.get(node.id)! > degrees.get(anchor.id)!) anchor = node;
  }
  const maxDegree = degrees.get(anchor.id)!;
  const repositoryDirections = new Map<string, GraphPoint>();
  for (const node of nodes) {
    if (node.id === anchor.id) {
      positions.set(node.id, { x: 0, y: 0, z: 0 });
      continue;
    }
    if (!repositoryDirections.has(node.repositoryId)) repositoryDirections.set(node.repositoryId, sphereDirection(`repository:${node.repositoryId}`));
    const bias = repositoryDirections.get(node.repositoryId)!;
    const direction = sphereDirection(node.id);
    const x = direction.x * .86 + bias.x * .14;
    const y = direction.y * .86 + bias.y * .14;
    const z = direction.z * .86 + bias.z * .14;
    const length = Math.hypot(x, y, z);
    const prominence = maxDegree ? degrees.get(node.id)! / maxDegree : 0;
    // Cube-root radii fill volume; degree pulls connected memories inward without flattening a repository.
    const radius = Math.cbrt(.18 ** 3 + (.94 ** 3 - .18 ** 3) * hash(`${node.id}:radius`)) * (1 - .24 * prominence);
    positions.set(node.id, { x: x / length * radius, y: y / length * radius, z: z / length * radius });
  }
  return positions;
}

export function stepGraphRange(range: GraphRange, direction: -1 | 0 | 1): GraphRange {
  return Math.max(0, Math.min(4, range + direction)) as GraphRange;
}

export function createWheelGesture() {
  let lastWheel = -Infinity;
  let lastFineWheel = -Infinity;
  return {
    step(event: { deltaX: number; deltaY: number; deltaMode: number }, now: number): -1 | 0 | 1 {
      if (!event.deltaY || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return 0;
      const amount = Math.abs(event.deltaY);
      const pause = now - lastWheel;
      lastWheel = now;
      // Discrete mouse notches can repeat; pixel-mode trackpad inertia belongs to one gesture.
      const notch = event.deltaMode !== 0 || ((amount === 100 || amount === 120) && now - lastFineWheel > 180);
      if (!notch) {
        lastFineWheel = now;
        if (pause < 180) return 0;
      }
      return event.deltaY > 0 ? 1 : -1;
    },
    reset() {
      lastWheel = -Infinity;
      lastFineWheel = -Infinity;
    },
  };
}
