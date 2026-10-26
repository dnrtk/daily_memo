// memo-core の serde 型に対応する TypeScript 型

export type FileKind = "text" | "document" | "other";

export interface DirEntry {
  name: string;
  path: string;
  is_dir: boolean;
  kind: FileKind | null;
  category_id: string | null;
  size: number;
  modified: number | null;
}

export type Encoding = "utf8" | "utf8-bom" | "shift-jis";
export type Eol = "lf" | "crlf";

export interface TextFile {
  content: string;
  encoding: Encoding;
  eol: Eol;
}

export type SaveMode = "new_file" | "append";

export interface Category {
  id: string;
  name: string;
  dir: string;
  description: string;
  examples: string[];
  filename: string;
  mode: SaveMode;
  file_template?: string | null;
  entry_template?: string | null;
}

export interface CategoryConfig {
  version: number;
  daily_category: string | null;
  categories: Category[];
}

export interface WorkspaceInfo {
  root: string;
  name: string;
  categories: CategoryConfig;
}

export interface Saved {
  path: string;
  category_id: string;
  appended: boolean;
}

export type ProviderKind = "ollama" | "openai_compat";

export interface Provider {
  id: string;
  kind: ProviderKind;
  base_url: string;
  api_key?: string | null;
  timeout_secs: number;
}

export interface ModelRef {
  provider: string;
  model: string;
}

export interface EmbeddingRef extends ModelRef {
  query_prefix?: string;
  document_prefix?: string;
}

export interface SearchSettings {
  use_llm_query: boolean;
  max_file_mb: number;
  exclude_dirs: string[];
}

export interface EditorSettings {
  autosave_ms: number;
  font_size: number;
  line_wrapping: boolean;
}

export interface TerminalSettings {
  shell: string;
}

export interface AppSettings {
  providers: Provider[];
  roles: { embedding: EmbeddingRef; chat: ModelRef };
  editor: EditorSettings;
  search: SearchSettings;
  terminal: TerminalSettings;
  last_workspace?: string | null;
}

// ---- 検索 ----------------------------------------------------------------------

export type Phase = "idle" | "scanning" | "extracting" | "embedding";

export interface Progress {
  phase: Phase;
  done: number;
  total: number;
  current: string | null;
}

export interface IndexStats {
  files: number;
  chunks: number;
  pending_embeddings: number;
  errors: number;
}

export type VectorState =
  | { state: "ready" }
  | { state: "model_mismatch"; indexed: string; configured: string }
  | { state: "unavailable"; message: string };

export interface ScanReport {
  scanned: number;
  indexed: number;
  unchanged: number;
  removed: number;
  failed: number;
  skipped_pdf: number;
}

export interface IndexStatus {
  root: string | null;
  progress: Progress;
  stats: IndexStats;
  vector: VectorState;
  pdf_note: string | null;
  last_report: ScanReport | null;
  last_error: string | null;
  failures: [string, string][];
}

export interface QueryPlan {
  query: string;
  search_text: string;
  keywords: string[];
  file_kinds: string[];
  date_from: string | null;
  date_to: string | null;
  categories: string[];
  category_dirs: string[];
  by_llm: boolean;
  note: string | null;
}

export type MatchedBy = "keyword" | "semantic" | "both";

export interface ChunkHit {
  chunk_id: number;
  label: string;
  line: number | null;
  line_end: number | null;
  page: number | null;
  snippet: string;
  score: number;
  matched_by: MatchedBy;
}

export interface FileHit {
  path: string;
  name: string;
  kind: string;
  modified: number;
  score: number;
  hits: ChunkHit[];
}

export interface SearchResult {
  plan: QueryPlan;
  results: FileHit[];
  terms: string[];
  used_vector: boolean;
  vector_note: string | null;
  elapsed_ms: number;
}

/** インデックス上のチャンク(文書の閲覧用) */
export interface ChunkRow {
  id: number;
  path: string;
  kind: string;
  mtime: number;
  label: string;
  line: number | null;
  page: number | null;
  text: string;
}
