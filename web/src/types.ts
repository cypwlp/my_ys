/** Shapes returned by crates/core.  Kept in one file so a Rust field rename shows
 *  up as a type error here rather than as `undefined` in the table. */

export type Format =
  | "Zip"
  | "SevenZip"
  | "Tar"
  | "GZip"
  | "BZip2"
  | "Xz"
  | "Zstd"
  | "TarGz"
  | "TarBz2"
  | "TarXz"
  | "TarZst"
  | "Rar"
  | "Split"
  | "Unknown";

export interface Entry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  packed: number | null;
  mtime: number | null;
  attrs: string;
  encrypted: boolean;
  codec: string | null;
  unsafePath: boolean;
}

export interface ArchiveInfo {
  path: string;
  format: Format;
  engine: string;
  entries: Entry[];
  totalSize: number;
  totalPacked: number;
  needsPassword: boolean;
  isSplit: boolean;
  warnings: string[];
}

export interface Progress {
  fraction: number | null;
  done: number;
  total: number;
  summary: string;
}

export interface Outcome {
  ok: boolean;
  /** The user stopped it.  Partial work is real, so this is not a failure. */
  cancelled: boolean;
  items: number;
  bytesOut: number;
  errors: string[];
  blocked: string[];
  engine: string;
}

export type Overwrite = "Ask" | "Always" | "Skip" | "KeepBoth" | "Newer";

export interface ExtractRequest {
  archive: string;
  dest: string;
  members: string[];
  password?: string;
  overwrite: Overwrite;
  repairNames: boolean;
  testOnly: boolean;
}

export interface CompressOptions {
  archive: string;
  sources: string[];
  format: Format;
  password?: string;
  headerEncryption: boolean;
  level: number;
  threads: number;
}

export interface EngineRow {
  id: string;
  kind: string;
  formats: [string, boolean][];
  path: string | null;
}

/** Progress events are pushed by the Rust side; the UI only ever needs percent + text. */
export type Listener = (percent: number, detail: string) => void;

export type JobState = "Running" | "Done" | "Failed" | "Cancelled";

export interface Job {
  id: number;
  /** "extract" | "compress" | "test" */
  kind: string;
  label: string;
  state: JobState;
  /** -1 when the engine cannot know the total. */
  percent: number;
  detail: string;
  items: number;
  bytes: number;
  blocked: number;
  error: string | null;
}

export interface Snapshot {
  jobs: Job[];
  /** Drained since the previous poll, so one job cannot be reacted to twice. */
  events: Job[];
  busy: boolean;
}

export interface Settings {
  theme: "dark" | "light" | "system";
  skin: "material" | "fluent";
  defaultFormat: Format;
  level: number;
  threads: number;
  overwrite: Overwrite;
  repairNames: boolean;
  openAfterExtract: boolean;
  useRecycleBin: boolean;
  history: string[];
}
