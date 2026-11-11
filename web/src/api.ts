import { invoke } from "@tauri-apps/api/core";
import type {
  ArchiveInfo,
  CompressOptions,
  EngineRow,
  Entry,
  ExtractRequest,
  Job,
  Listener,
  Settings,
  Snapshot,
} from "./types";

/** True inside the Tauri shell; false when the same UI is opened in a browser. */
export const inTauri = "__TAURI_INTERNALS__" in window;

const mockEntryFile = (path: string, size: number, packed: number | null, extra: Partial<Entry> = {}): Entry => ({
  name: path.replace(/\/$/, "").split("/").slice(-1)[0],
  path,
  isDir: path.endsWith("/"),
  size,
  packed,
  mtime: 1793866263,
  attrs: path.endsWith("/") ? "D" : "A",
  encrypted: false,
  codec: null,
  unsafePath: false,
  ...extra,
});

/** Rendered in a plain browser (`npm run dev`) so the UI can be reviewed without a
 *  Rust build.  Deliberately nested: a folder two levels deep, an inner archive, an
 *  encrypted member and a traversal row -- the four rows whose rendering has broken
 *  before. */
export const mockArchive: ArchiveInfo = {
  path: "D:/demo/demo.zip",
  format: "Zip",
  engine: "zip-managed",
  needsPassword: false,
  isSplit: false,
  warnings: [],
  totalSize: 81000,
  totalPacked: 4096,
  entries: [
    mockEntryFile("圖片/", 0, null),
    mockEntryFile("圖片/截圖 2026-11-05.png", 30720, 29000),
    mockEntryFile("圖片/圖表/季報.svg", 23240, 8100),
    mockEntryFile("報告表2026/", 0, null, { codec: "GB18030" }),
    mockEntryFile("報告表2026/季報.xlsx", 25800, 21000, { codec: "GB18030" }),
    mockEntryFile("報告表2026/內層包.zip", 4096, 4096),
    mockEntryFile("README.txt", 1234, 45),
    mockEntryFile("機密.7z", 900, 880, { encrypted: true, attrs: "AEM" }),
    mockEntryFile("../../外洩/", 0, null, { unsafePath: true, codec: "GB18030" }),
    mockEntryFile("../../外洩/pwned.txt", 12, 12, { unsafePath: true }),
  ],
};

export const defaultSettings: Settings = {
  theme: "system",
  skin: "material",
  defaultFormat: "SevenZip",
  level: 5,
  threads: 0,
  overwrite: "KeepBoth",
  repairNames: true,
  openAfterExtract: true,
  useRecycleBin: true,
  history: ["D:/demo/demo.zip", "D:/下載/軟體包.7z"],
};

// ---------------------------------------------------------------- preview mode

let mockJobs: Job[] = [];
let mockNextId = 1;
/** Cancelled ids, so the simulated ticker cannot overwrite the terminal state. */
const mockCancelled = new Set<number>();

function mockStart(kind: string, label: string): number {
  const id = mockNextId++;
  mockJobs.push({ id, kind, label, state: "Running", percent: -1, detail: "進行中", items: 0, bytes: 0, blocked: 0, error: null });
  const started = Date.now();
  const tick = setInterval(() => {
    const job = mockJobs.find((j) => j.id === id);
    if (!job || mockCancelled.has(id)) return clearInterval(tick);
    const pct = Math.min(100, ((Date.now() - started) / 2400) * 100);
    job.percent = pct;
    job.detail = `模擬 ${Math.round(pct)}%`;
    if (pct >= 100) {
      job.state = "Done";
      job.items = 3;
      job.bytes = 81_000;
      job.detail = "完成";
      clearInterval(tick);
    }
  }, 180);
  return id;
}

function mockSnapshot(): Snapshot {
  const busy = mockJobs.some((j) => j.state === "Running");
  return { jobs: [...mockJobs], events: [], busy };
}

// ------------------------------------------------------------------- commands

export const api = {
  /** The path the shell handed us on the command line, consumed once. */
  startupPath(): Promise<string | null> {
    if (!inTauri) return Promise.resolve(null);
    return invoke<string | null>("take_startup_path");
  },

  async open(archive: string, password?: string): Promise<ArchiveInfo> {
    if (!inTauri) return { ...mockArchive, path: archive };
    return invoke<ArchiveInfo>("open_archive", { archive, password: password ?? null });
  },

  async openMember(archive: string, member: string, password?: string): Promise<ArchiveInfo> {
    if (!inTauri) {
      return {
        ...mockArchive,
        path: `C:/Users/you/AppData/Local/Temp/quickarc-demo/${member}`,
        entries: [mockEntryFile("deep/hello.txt", 14, 14)],
      };
    }
    return invoke<ArchiveInfo>("open_member", { archive, member, password: password ?? null });
  },

  /** Returns the job id; progress arrives through `snapshot`. */
  async startExtract(req: ExtractRequest): Promise<number> {
    if (!inTauri) return mockStart(req.testOnly ? "test" : "extract", req.archive.split(/[/\\]/).pop() ?? req.archive);
    return invoke<number>("start_extract", { req });
  },

  async startCompress(opts: CompressOptions): Promise<number> {
    if (!inTauri) return mockStart("compress", opts.archive.split(/[/\\]/).pop() ?? opts.archive);
    return invoke<number>("start_compress", { opts });
  },

  snapshot(): Promise<Snapshot> {
    if (!inTauri) return Promise.resolve(mockSnapshot());
    return invoke<Snapshot>("job_snapshot");
  },

  cancel(id: number): Promise<boolean> {
    if (!inTauri) {
      mockCancelled.add(id);
      mockJobs = mockJobs.map((j) =>
        j.state === "Running" && j.id === id ? { ...j, state: "Cancelled" as const, error: "已取消", detail: "已取消" } : j,
      );
      return Promise.resolve(true);
    }
    return invoke<boolean>("cancel_job", { id });
  },

  clearJobs(): Promise<number> {
    if (!inTauri) {
      mockJobs = mockJobs.filter((j) => j.state === "Running");
      return Promise.resolve(0);
    }
    return invoke<number>("clear_jobs");
  },

  engines(): Promise<EngineRow[]> {
    if (!inTauri) {
      return Promise.resolve([
        { id: "tar-managed", kind: "Managed", formats: [["tar", true], ["tar.gz", true], ["tar.bz2", true], ["tar.xz", false], ["tar.zst", false]], path: null },
        { id: "zip-managed", kind: "Managed", formats: [["ZIP", true]], path: null },
        { id: "codec-managed", kind: "Managed", formats: [["gzip", false], ["bzip2", false], ["xz", false], ["zstd", false]], path: null },
      ]);
    }
    return invoke<EngineRow[]>("list_engines");
  },

  hash(path: string, algo: string): Promise<string> {
    if (!inTauri) return Promise.resolve("(瀏覽器預覽不執行雜湊)");
    return invoke<string>("hash_file", { path, algo });
  },

  settings(): Promise<Settings> {
    if (!inTauri) return Promise.resolve(defaultSettings);
    return invoke<Settings>("get_settings");
  },

  saveSettings(next: Settings): Promise<void> {
    if (!inTauri) return Promise.resolve();
    return invoke<void>("set_settings", { settings: next });
  },

  clearHistory(): Promise<void> {
    if (!inTauri) return Promise.resolve();
    return invoke<void>("clear_history");
  },

  shellPreview(): Promise<string> {
    if (!inTauri) return Promise.resolve("; 瀏覽器預覽不会產生 .reg 內容");
    return invoke<string>("shell_preview_text");
  },

  shellInstalled(): Promise<boolean> {
    if (!inTauri) return Promise.resolve(false);
    return invoke<boolean>("shell_is_installed");
  },

  recycle(paths: string[]): Promise<void> {
    if (!inTauri) return Promise.resolve();
    return invoke<void>("recycle_paths", { paths });
  },
};

export type { Listener };
