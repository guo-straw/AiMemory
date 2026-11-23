export interface Repository {
  id: string;
  name: string;
  remote: string;
  memoryCount: number;
}

export interface Workspace {
  root: string;
  repositories: Repository[];
  warnings: string[];
}

export interface RepositoryInput {
  name: string;
  remote: string;
}

export interface MemorySummary {
  id: string;
  repositoryId: string;
  title: string;
  summary: string;
  kind: string;
  tags: string[];
  updatedAt: string;
  path: string;
  embeddingStatus: 'unconfigured' | 'pending' | 'processing' | 'ready' | 'failed';
  embeddingError: string | null;
}

export interface MemoryRecord extends MemorySummary {
  markdown: string;
  relatedIds: string[];
  outgoingRelatedIds: string[];
  codePaths: string[];
  source: string | null;
  revision: string;
  currentConclusion: string;
  conclusionOwner: 'auto' | 'manual';
  suggestions: { sourceId: string; summary: string }[];
  evidenceSources: EvidenceSource[];
}

export interface EvidenceSource {
  sourceId: string;
  sessionId: string;
  turnId: string;
  messageRole: string | null;
  status: 'located' | 'unresolved' | 'section_only';
  startLine: number | null;
  endLine: number | null;
  quote?: string;
}

export interface MemoryGraphEdge {
  source: string;
  target: string;
}

export interface MemoryGraph {
  nodes: MemorySummary[];
  edges: MemoryGraphEdge[];
}

export interface MemoryInput {
  id?: string;
  repositoryId: string;
  title: string;
  kind: string;
  tags: string[];
  markdown: string;
  relatedIds: string[];
  revision?: string;
  source?: string | null;
}

export type ServiceProvider = 'openai' | 'deepseek' | 'custom';

export interface ServiceConfig {
  provider?: ServiceProvider | null;
  baseUrl: string;
  model: string;
  hasApiKey: boolean;
}

export interface ServiceInput {
  provider?: ServiceProvider;
  baseUrl: string;
  model: string;
  apiKey?: string;
}

export interface AppSettings {
  chat: ServiceConfig;
  embedding: ServiceConfig;
  codexDirectory: string;
  archiveEnabled: boolean;
  archiveStartedAt: string | null;
  runInBackground: boolean;
}

export interface SettingsInput {
  chat: ServiceInput;
  embedding: ServiceInput;
  codexDirectory: string;
  archiveEnabled: boolean;
  runInBackground: boolean;
}

export interface ConnectionTest {
  message: string;
  latencyMs: number;
  inputTokens: number;
  outputTokens: number;
  usageReported: boolean;
}

export interface ArchiveTurn {
  sourceId: string;
  repository: string;
  sessionId: string;
  turnId: string;
  completedAt: string;
  status: 'pending' | 'extracting' | 'saved' | 'failed' | 'skipped';
  memoryCount: number;
  chatInputTokens: number;
  chatOutputTokens: number;
  embeddingInputTokens: number;
  chatLatencyMs: number | null;
  embeddingLatencyMs: number | null;
  error: string | null;
  usageIncomplete: boolean;
}

export interface ArchiveStatus {
  enabled: boolean;
  startedAt: string | null;
  scanning: boolean;
  processedTurns: number;
  writtenMemories: number;
  memoryWrites: number;
  pendingEmbeddings: number;
  failedEmbeddings: number;
  failedEmbeddingDetails: { memoryId: string; error: string | null }[];
  processingEmbeddings: number;
  chatInputTokens: number;
  chatOutputTokens: number;
  embeddingInputTokens: number;
  recentTurns: ArchiveTurn[];
  lastError: string | null;
  usageIncomplete: boolean;
}

export interface SearchRequest {
  query?: string;
  repositoryId: string | null;
  limit?: number;
  mode?: 'auto' | 'keyword' | 'id';
  groupOf?: string;
  offset?: number;
}

export interface SearchHit extends MemorySummary {
  score: number;
  groupCount: number;
  matchedSections: { title: string; status: 'current' | 'historical' | 'suggested' }[];
  matchesHistoryOnly: boolean;
}

export interface SearchResponse {
  results: SearchHit[];
  queryVectorCached: boolean;
  queryUsageReported: boolean;
  embeddingInputTokens: number;
  embeddingLatencyMs: number;
  localSearchMs: number;
  pendingEmbeddings: number;
  searchMode: 'keyword' | 'semantic' | 'hybrid' | 'id' | 'group';
  scoreKind: 'text' | 'cosine' | 'rrf' | 'id';
  fallbackReason?: string;
  groupOf?: string;
  offset?: number;
  total?: number;
  hasMore?: boolean;
}

export interface McpInfo {
  dataDirectory: string;
  vaultRoot: string;
}

export interface McpHttpStatus {
  running: boolean;
  url: string;
}

export interface AppUpdateResult {
  latestVersion: string;
  status: 'available' | 'current' | 'ahead';
  releaseUrl: string;
  installer: UpdateInstaller | null;
}

export interface UpdateInstaller {
  assetId: number;
  name: string;
  size: number;
}
