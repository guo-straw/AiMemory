import type { MemoryGraphEdge, MemorySummary } from './types';
import type { GraphScope, GraphPoint } from './graph';

interface SceneNode extends MemorySummary { position: GraphPoint; degree: number; freshness: number }
interface ProjectedNode { node: SceneNode; x: number; y: number; depth: number; radius: number; color: string; alpha: number; focused: boolean; matched: boolean }
interface LabelBox { id: string; x: number; y: number; width: number; height: number }
export interface StarAppearance {
  selectedId: string | null;
  hoveredId: string | null;
  colors: Map<string, string>;
  colorByRepository: boolean;
  nodeSize: number;
  lineWidth: number;
  labelDensity: '低' | '标准' | '高';
  panelOpen: boolean;
  showLinks: boolean;
  searchQuery: string;
}

export class StarMapRenderer {
  private context: CanvasRenderingContext2D;
  private frame = 0;
  private active = true;
  private nodes: SceneNode[] = [];
  private edges: MemoryGraphEdge[] = [];
  private adjacency = new Map<string, Set<string>>();
  private projected: ProjectedNode[] = [];
  private labels: LabelBox[] = [];
  private anchorId: string | null = null;
  private yaw = -.28;
  private pitch = .2;
  private zoom = 1;
  private panX = 0;
  private panY = 0;
  private sprites = new Map<string, HTMLCanvasElement>();
  private backdrop = document.createElement('canvas');
  private backdropKey = '';
  private palette = { accent: '#7aa5ff', foreground: '#e5edfc', muted: '#99adc8', faint: '#6f829f', line: '#7592b7', background: '#080f1d', font: '-apple-system, BlinkMacSystemFont, sans-serif' };
  private appearance: StarAppearance = { selectedId: null, hoveredId: null, colors: new Map(), colorByRepository: true, nodeSize: 100, lineWidth: 100, labelDensity: '标准', panelOpen: false, showLinks: true, searchQuery: '' };

  constructor(private canvas: HTMLCanvasElement) {
    this.context = canvas.getContext('2d')!;
    this.refreshTheme();
  }

  refreshTheme() {
    const style = getComputedStyle(this.canvas);
    const color = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
    this.palette = {
      accent: color('--accent', '#7aa5ff'), foreground: color('--fg', '#e5edfc'), muted: color('--muted', '#99adc8'), faint: color('--faint', '#6f829f'),
      line: color('--graph-line', '#7592b7'), background: color('--graph-bg', '#080f1d'), font: color('--font', '-apple-system, BlinkMacSystemFont, sans-serif'),
    };
    this.backdropKey = '';
    this.requestDraw();
  }

  setScene(scope: GraphScope, positions: Map<string, GraphPoint>, degrees: Map<string, number>, adjacency: Map<string, Set<string>>) {
    const dates = scope.nodes.map(node => Date.parse(node.updatedAt));
    const latest = dates.reduce((value, date) => Number.isFinite(date) ? Math.max(value, date) : value, 0);
    this.nodes = scope.nodes.map((node, index) => ({ ...node, position: positions.get(node.id)!, degree: degrees.get(node.id) ?? 0, freshness: Number.isFinite(dates[index]) ? .55 + .45 * Math.exp(-Math.max(0, latest - dates[index]) / (180 * 86400000)) : .7 }));
    this.anchorId = scope.range ? scope.centerId : this.nodes.find(node => node.position.x === 0 && node.position.y === 0 && node.position.z === 0)?.id ?? null;
    this.edges = scope.edges;
    this.adjacency = adjacency;
    this.requestDraw();
  }

  setAppearance(appearance: StarAppearance) { this.appearance = appearance; this.requestDraw(); }
  setActive(active: boolean) {
    this.active = active;
    if (active) this.requestDraw();
    else if (this.frame) { cancelAnimationFrame(this.frame); this.frame = 0; }
  }
  requestDraw = () => { if (this.active && !this.frame) this.frame = requestAnimationFrame(() => this.draw()); };
  rotate(dx: number, dy: number) { this.yaw += dx * .006; this.pitch = Math.max(-1.4, Math.min(1.4, this.pitch + dy * .006)); this.requestDraw(); }
  pan(dx: number, dy: number) { this.panX += dx; this.panY += dy; this.requestDraw(); }
  zoomBy(factor: number) { this.zoom = Math.max(.35, Math.min(4, this.zoom * factor)); this.requestDraw(); return Math.round(this.zoom * 100); }
  reset() { this.yaw = -.28; this.pitch = .2; this.zoom = 1; this.panX = 0; this.panY = 0; this.requestDraw(); return 100; }

  hit(clientX: number, clientY: number) {
    const rect = this.canvas.getBoundingClientRect();
    const x = clientX - rect.left, y = clientY - rect.top;
    const label = this.labels.find(label => x >= label.x && x <= label.x + label.width && y >= label.y && y <= label.y + label.height);
    if (label) return label.id;
    for (let index = this.projected.length - 1; index >= 0; index--) {
      const point = this.projected[index];
      const distance = Math.hypot(point.x - x, point.y - y);
      if (distance <= Math.max(9, point.radius + 4)) return point.node.id;
    }
    return null;
  }
  destroy() { if (this.frame) cancelAnimationFrame(this.frame); this.frame = 0; this.sprites.clear(); }

  private drawBackdrop(width: number, height: number) {
    const key = `${width}:${height}:${this.palette.background}`;
    if (key !== this.backdropKey) {
      this.backdropKey = key;
      this.backdrop.width = Math.round(width); this.backdrop.height = Math.round(height);
      const context = this.backdrop.getContext('2d')!;
      context.fillStyle = this.palette.background; context.fillRect(0, 0, width, height);
      const cloud = context.createRadialGradient(width * .46, height * .51, 0, width * .46, height * .51, Math.max(width, height) * .52);
      cloud.addColorStop(0, '#172a4838'); cloud.addColorStop(.6, '#11213914'); cloud.addColorStop(1, '#080f1d00');
      context.fillStyle = cloud; context.fillRect(0, 0, width, height);
      let random = 724031;
      for (let index = 0; index < 260; index++) {
        random = Math.imul(random, 1664525) + 1013904223; const x = (random >>> 0) / 4294967296 * width;
        random = Math.imul(random, 1664525) + 1013904223; const y = (random >>> 0) / 4294967296 * height;
        context.globalAlpha = index % 17 === 0 ? .32 : .06 + (index % 5) * .025;
        context.fillStyle = index % 3 ? '#a8c2e5' : '#a0dad2';
        context.beginPath(); context.arc(x, y, index % 17 === 0 ? 1 : .5, 0, Math.PI * 2); context.fill();
      }
    }
    this.context.drawImage(this.backdrop, 0, 0, width, height);
  }

  private starSprite(color: string) {
    const cached = this.sprites.get(color);
    if (cached) return cached;
    const sprite = document.createElement('canvas'); sprite.width = sprite.height = 80;
    const context = sprite.getContext('2d')!;
    const glow = context.createRadialGradient(40, 40, 0, 40, 40, 40);
    glow.addColorStop(0, '#ffffff'); glow.addColorStop(.07, '#f3f7ff'); glow.addColorStop(.17, color);
    glow.addColorStop(.38, color + '65'); glow.addColorStop(.7, color + '17'); glow.addColorStop(1, color + '00');
    context.fillStyle = glow; context.fillRect(0, 0, 80, 80);
    this.sprites.set(color, sprite);
    return sprite;
  }

  private draw() {
    this.frame = 0;
    const width = this.canvas.clientWidth, height = this.canvas.clientHeight;
    if (!this.active || !width || !height) return;
    const started = performance.now();
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    if (this.canvas.width !== Math.round(width * dpr) || this.canvas.height !== Math.round(height * dpr)) { this.canvas.width = Math.round(width * dpr); this.canvas.height = Math.round(height * dpr); }
    const ctx = this.context;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.globalAlpha = 1;
    this.drawBackdrop(width, height);
    const { selectedId, hoveredId, panelOpen, searchQuery } = this.appearance;
    const emphasis = hoveredId ?? selectedId;
    const neighbors = this.adjacency.get(emphasis ?? '') ?? new Set<string>();
    const term = searchQuery.trim().toLocaleLowerCase();
    const narrow = width < 760;
    const roomWidth = width - (panelOpen && !narrow ? 370 : 0);
    const top = narrow ? 146 : 110;
    const bottom = narrow && panelOpen ? 63 + Math.min(height * .42, 280) + 18 : 90;
    const roomHeight = Math.max(40, height - top - bottom);
    const centerX = roomWidth * .5 + this.panX, centerY = top + roomHeight * .48 + this.panY;
    const scale = Math.max(55, Math.min(roomWidth * .43, roomHeight * .47)) * this.zoom;
    const cy = Math.cos(this.yaw), sy = Math.sin(this.yaw), cp = Math.cos(this.pitch), sp = Math.sin(this.pitch);
    this.projected = this.nodes.map(node => {
      const x = node.position.x * cy + node.position.z * sy, z = node.position.z * cy - node.position.x * sy;
      const y = node.position.y * cp - z * sp, depth = z * cp + node.position.y * sp;
      const perspective = 3.5 / (3.5 - depth * .65);
      const focused = node.id === emphasis || neighbors.has(node.id);
      const matched = !term || `${node.title} ${node.id} ${node.summary} ${node.tags.join(' ')}`.toLocaleLowerCase().includes(term);
      const radius = Math.min(11, 2.8 + Math.sqrt(node.degree) * .95 + (node.id === this.anchorId ? 3.2 : 0)) * perspective * Math.sqrt(this.zoom) * this.appearance.nodeSize / 100;
      return { node, x: centerX + x * scale * perspective, y: centerY + y * scale * perspective, depth, radius,
        color: this.appearance.colorByRepository ? this.appearance.colors.get(node.repositoryId) ?? this.palette.accent : this.palette.accent,
        alpha: Math.min(1, (.65 + perspective * .22) * node.freshness) * (emphasis && !focused ? .16 : 1) * (!matched ? .13 : 1), focused, matched };
    }).sort((left, right) => left.depth - right.depth);
    const byId = new Map(this.projected.map(point => [point.node.id, point]));
    if (this.appearance.showLinks) this.drawLinks(byId, emphasis);
    this.drawStars(emphasis);
    this.drawLabels(roomWidth, top, height - 80, emphasis);
    ctx.globalAlpha = 1;
    this.canvas.dataset.renderMs = (performance.now() - started).toFixed(2);
    this.canvas.dataset.drawCount = String(Number(this.canvas.dataset.drawCount ?? 0) + 1);
  }

  private drawLinks(byId: Map<string, ProjectedNode>, emphasis: string | null) {
    const ctx = this.context;
    for (const focused of [false, true]) for (const crossRepository of [false, true]) {
      ctx.beginPath();
      for (const edge of this.edges) {
        const from = byId.get(edge.source)!, to = byId.get(edge.target)!;
        if ((edge.source === emphasis || edge.target === emphasis) !== focused || (from.node.repositoryId !== to.node.repositoryId) !== crossRepository) continue;
        if (this.appearance.searchQuery.trim() && !from.matched && !to.matched) continue;
        ctx.moveTo(from.x, from.y); ctx.lineTo(to.x, to.y);
      }
      ctx.strokeStyle = crossRepository ? '#d2a46f' : focused ? this.palette.accent : this.palette.line;
      ctx.lineWidth = (focused ? 1.5 : .65) * this.appearance.lineWidth / 100;
      ctx.globalAlpha = focused ? .9 : emphasis ? .035 : .21; ctx.stroke();
    }
    ctx.globalAlpha = 1;
  }

  private drawStars(emphasis: string | null) {
    const ctx = this.context;
    for (const point of this.projected) {
      if (point.x < -40 || point.x > this.canvas.clientWidth + 40 || point.y < -40 || point.y > this.canvas.clientHeight + 40) continue;
      const selected = point.node.id === emphasis;
      const size = point.radius * (selected ? 9 : 6.5);
      ctx.globalAlpha = point.alpha;
      ctx.drawImage(this.starSprite(point.color), point.x - size / 2, point.y - size / 2, size, size);
      if (selected) {
        ctx.strokeStyle = point.color; ctx.lineWidth = .8; ctx.globalAlpha = .65;
        ctx.beginPath(); ctx.arc(point.x, point.y, point.radius * 2, 0, Math.PI * 2); ctx.stroke();
      }
    }
    ctx.globalAlpha = 1;
  }

  private drawLabels(width: number, top: number, bottom: number, emphasis: string | null) {
    const ctx = this.context;
    const maximum = this.appearance.labelDensity === '低' ? 3 : this.appearance.labelDensity === '高' ? 28 : 9;
    const priority = (point: ProjectedNode) => point.node.id === emphasis ? 1e9 : point.focused ? 1e6 + point.node.degree : point.matched && this.appearance.searchQuery ? 1e5 + point.node.degree : point.node.degree;
    const ordered = [...this.projected].sort((left, right) => priority(right) - priority(left) || right.depth - left.depth);
    this.labels = [];
    for (const point of ordered.slice(0, maximum * 10)) {
      if (this.labels.length === maximum) break;
      if (point.alpha < .2 || point.x < 15 || point.x > width - 15 || point.y < top || point.y > bottom - 24) continue;
      const selected = point.node.id === emphasis;
      ctx.font = `${selected ? 550 : 400} ${selected ? 13 : 11}px ${this.palette.font}`;
      const limit = Math.min(210, width * .6);
      let text = point.node.title;
      while (text.length > 1 && ctx.measureText(text).width > limit) text = text.slice(0, -1);
      if (text !== point.node.title) text = text.slice(0, -1) + '…';
      const boxWidth = ctx.measureText(text).width + 16, boxHeight = selected ? 41 : 24;
      const left = Math.max(8, Math.min(width - boxWidth - 8, point.x - boxWidth / 2)), y = point.y + point.radius + 10;
      if (this.labels.some(box => left < box.x + box.width + 5 && left + boxWidth > box.x - 5 && y < box.y + box.height + 3 && y + boxHeight > box.y - 3)) continue;
      if (selected) { ctx.fillStyle = '#111e32ed'; ctx.beginPath(); ctx.roundRect(left, y, boxWidth, boxHeight, 6); ctx.fill(); }
      ctx.globalAlpha = selected ? 1 : .78; ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
      ctx.fillStyle = selected ? this.palette.foreground : this.palette.muted; ctx.fillText(text, left + boxWidth / 2, y + 12);
      if (selected) { ctx.font = '10px ui-monospace, Menlo, monospace'; ctx.fillStyle = this.palette.faint; ctx.fillText(point.node.id, left + boxWidth / 2, y + 30); }
      this.labels.push({ id: point.node.id, x: left, y, width: boxWidth, height: boxHeight });
    }
    ctx.globalAlpha = 1;
  }
}
