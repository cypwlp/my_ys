import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AppBar,
  Box,
  Breadcrumbs,
  Button,
  Chip,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Divider,
  IconButton,
  Link,
  Menu,
  MenuItem,
  Snackbar,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableRow,
  TextField,
  Toolbar,
  Tooltip,
  Typography,
} from "@mui/material";
import {
  FolderOutlined,
  DownloadOutlined,
  UploadOutlined,
  CheckCircleOutline,
  ArrowUpwardOutlined,
  Refresh,
  SettingsOutlined,
  Search,
  ArchiveOutlined,
  InsertDriveFileOutlined,
  FolderZipOutlined,
  LockOutlined,
  WarningAmber,
} from "@mui/icons-material";
import { api, defaultSettings, inTauri, mockArchive } from "./api";
import { childrenOf } from "./browse";
import { humanRatio, humanSize, humanTime } from "./format";
import CompressDialog from "./CompressDialog";
import JobsPanel from "./JobsPanel";
import SettingsDialog from "./SettingsDialog";
import type { ArchiveInfo, Entry, Job, Settings } from "./types";

type SortKey = "name" | "size" | "packed" | "ratio" | "mtime" | "attrs";

const COLUMNS: { key: SortKey; label: string; width: number; align: "left" | "right" | "center" }[] = [
  { key: "name", label: "名稱", width: 0, align: "left" },
  { key: "size", label: "體積", width: 124, align: "right" },
  { key: "packed", label: "壓縮後", width: 124, align: "right" },
  { key: "ratio", label: "比率", width: 84, align: "right" },
  { key: "mtime", label: "修改日期", width: 152, align: "left" },
  { key: "attrs", label: "屬性", width: 72, align: "center" },
];

const INNER_ARCHIVE = /\.(zip|7z|tar|gz|bz2|xz|zst|tgz|rar|jar|apk|cab|iso)$/i;

/** One level of navigation.  Folders are pure state; an inner archive carries its
 *  own source because the Rust side materialises it into a scratch file. */
interface Level {
  label: string;
  source: string;
  prefix: string;
}

const baseName = (p: string) => p.split(/[/\\]/).filter(Boolean).slice(-1)[0] ?? p;

/** The full ids belong in the settings dialog; on the chip they wrap the toolbar. */
const SHORT_ID: Record<string, string> = {
  "tar-managed": "tar",
  "zip-managed": "zip",
  "codec-managed": "gz/bz2/xz/zst",
  "7zip": "7z",
};

function childIcon(e: Entry) {
  if (e.unsafePath) return <WarningAmber fontSize="small" color="warning" />;
  if (e.isDir) return <FolderOutlined fontSize="small" />;
  if (e.encrypted) return <LockOutlined fontSize="small" />;
  if (INNER_ARCHIVE.test(e.name)) return <FolderZipOutlined fontSize="small" />;
  return <InsertDriveFileOutlined fontSize="small" />;
}

export default function App() {
  const [settings, setSettings] = useState<Settings>(defaultSettings);
  const [info, setInfo] = useState<ArchiveInfo | null>(inTauri ? null : mockArchive);
  const [levels, setLevels] = useState<Level[]>(
    inTauri ? [] : [{ label: "demo.zip", source: mockArchive.path, prefix: "" }],
  );
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<{ key: SortKey; desc: boolean }>({ key: "name", desc: false });
  const [jobs, setJobs] = useState<Job[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [engineChip, setEngineChip] = useState("偵測中…");
  const [menu, setMenu] = useState<{ anchor: HTMLElement; paths: string[] } | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [askPw, setAskPw] = useState<{ resolve: (pw: string | null) => void } | null>(null);
  const [pwInput, setPwInput] = useState("");
  const [showCompress, setShowCompress] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [extractTo, setExtractTo] = useState<{ resolve: (dir: string | null) => void } | null>(null);
  const [dirInput, setDirInput] = useState("");
  const lastIndex = useRef(-1);
  const pollTimer = useRef<number | null>(null);

  const current = levels[levels.length - 1];

  useEffect(() => {
    void api.settings().then(setSettings);
    if (!inTauri) {
      setEngineChip("瀏覽器預覽（未接 Rust）");
      return;
    }
    api
      .engines()
      .then((rows) => setEngineChip(rows.map((r) => SHORT_ID[r.id] ?? r.id).join(" · ") || "無引擎"))
      .catch((e) => setEngineChip(`引擎錯誤：${e}`));
  }, []);

  /** Jobs are polled, not pushed: a blocking command cannot report progress and a
   *  flood of per-file events would stall the webview on a 100k-entry archive. */
  const poll = useCallback(async () => {
    const snap = await api.snapshot();
    setJobs(snap.jobs);
    for (const event of snap.events) {
      if (event.state === "Done") setNotice(`${event.label} 完成 · ${event.items} 項 · ${humanSize(event.bytes)}${event.blocked ? ` · 阻擋 ${event.blocked}` : ""}`);
      if (event.state === "Failed") setNotice(`${event.label} 失敗：${event.error ?? "未知錯誤"}`);
    }
    return snap.busy;
  }, []);

  useEffect(() => {
    if (pollTimer.current !== null) {
      window.clearInterval(pollTimer.current);
      pollTimer.current = null;
    }
    if (!jobs.some((j) => j.state === "Running")) return;
    pollTimer.current = window.setInterval(() => {
      void poll().then((busy) => {
        if (!busy && pollTimer.current !== null) {
          window.clearInterval(pollTimer.current);
          pollTimer.current = null;
        }
      });
    }, 200);
    return () => {
      if (pollTimer.current !== null) {
        window.clearInterval(pollTimer.current);
        pollTimer.current = null;
      }
    };
  }, [jobs, poll]);

  const openPath = useCallback(async (path: string, password?: string): Promise<boolean> => {
    try {
      const loaded = await api.open(path, password);
      setInfo(loaded);
      setLevels([{ label: baseName(loaded.path), source: path, prefix: "" }]);
      setSearch("");
      setSelected([]);
      setNotice(null);
      return true;
    } catch (e) {
      const text = String(e);
      if (/密碼/.test(text) && password === undefined) {
        const given = await new Promise<string | null>((resolve) => setAskPw({ resolve }));
        return given ? openPath(path, given) : false;
      }
      setNotice(`開啟失敗：${text}`);
      return false;
    }
  }, []);

  /** The Explorer verb or "open with" handed us a path before the page existed. */
  useEffect(() => {
    void api.startupPath().then((p) => {
      if (p) void openPath(p);
    });
  }, [openPath]);

  useEffect(() => {
    if (!inTauri) return;
    let stop: (() => void) | undefined;
    (async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      stop = await getCurrentWindow().onDragDropEvent((event) => {
        if (event.payload.type === "drop" && event.payload.paths.length > 0) void openPath(event.payload.paths[0]);
      });
    })();
    return () => stop?.();
  }, [openPath]);

  const pickFile = useCallback(async () => {
    if (!inTauri) {
      setInfo(mockArchive);
      setLevels([{ label: "demo.zip", source: mockArchive.path, prefix: "" }]);
      return;
    }
    const { open } = await import("@tauri-apps/plugin-dialog");
    const choice = await open({
      multiple: false,
      filters: [{ name: "壓縮包", extensions: ["7z", "zip", "tar", "gz", "bz2", "xz", "zst", "tgz", "rar"] }],
    });
    if (typeof choice === "string") void openPath(choice);
  }, [openPath]);

  const view = useMemo(() => {
    if (!info || !current) return [];
    const needle = search.trim().toLowerCase();
    const all = childrenOf(info.entries, current.prefix);
    return needle
      ? all.filter((e) => e.name.toLowerCase().includes(needle) || e.path.toLowerCase().includes(needle))
      : all;
  }, [info, current, search]);

  const sorted = useMemo(() => {
    const dir = sort.desc ? -1 : 1;
    const ratio = (e: Entry) => (e.packed === null || e.size <= 0 ? 0 : (e.size - e.packed) / e.size);
    return [...view].sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      switch (sort.key) {
        case "size":
          return dir * (a.size - b.size);
        case "packed":
          return dir * ((a.packed ?? a.size) - (b.packed ?? b.size));
        case "ratio":
          return dir * (ratio(a) - ratio(b));
        case "mtime":
          return dir * ((a.mtime ?? 0) - (b.mtime ?? 0));
        case "attrs":
          return dir * a.attrs.localeCompare(b.attrs);
        default:
          return dir * a.name.localeCompare(b.name, "zh-Hant");
      }
    });
  }, [view, sort]);

  const enter = useCallback(
    async (e: Entry) => {
      if (!info || !current) return;
      if (e.isDir) {
        setLevels((ls) => [...ls, { label: e.name, source: current.source, prefix: e.path }]);
        setSelected([]);
        return;
      }
      if (!INNER_ARCHIVE.test(e.name)) {
        setNotice(`雙擊只能進入資料夾或內層壓縮包；${e.name} 請先解壓`);
        return;
      }
      try {
        const loaded = await api.openMember(current.source, e.path);
        setInfo(loaded);
        setLevels((ls) => [...ls, { label: e.name, source: loaded.path, prefix: "" }]);
        setSelected([]);
      } catch (err) {
        setNotice(`內層開啟失敗：${err}`);
      }
    },
    [current, info],
  );

  const jumpTo = (index: number) => {
    if (index < 0 || index === levels.length - 1) return;
    const target = levels[index];
    const crossedArchive = levels.slice(index + 1).some((l) => l.source !== target.source);
    setLevels((ls) => ls.slice(0, index + 1));
    setSelected([]);
    if (crossedArchive) {
      void api
        .open(target.source)
        .then((loaded) => setInfo(loaded))
        .catch((e) => setNotice(`回到上層失敗：${e}`));
    }
  };

  const runExtract = useCallback(
    async (dest: string, members: string[], testOnly = false) => {
      if (!info) return;
      try {
        await api.startExtract({
          archive: info.path,
          dest,
          members,
          overwrite: settings.overwrite,
          repairNames: settings.repairNames,
          testOnly,
        });
        // One poll right away.  The interval only starts once a Running job is in
        // state, and a fast job can finish before anything asks for it.
        await poll();
        setNotice(testOnly ? "已送進工作佇列：測試完整性" : "已送進工作佇列");
      } catch (e) {
        setNotice(`無法開始工作：${e}`);
      }
    },
    [info, poll, settings],
  );

  const smartExtract = useCallback(async () => {
    if (!info) return;
    const members = selected.length > 0 ? selected : [];
    if (members.length === 0 && info.entries.length > 400) {
      // Many top-level entries usually means the archive lacks a single root folder;
      // asking is cheaper than dumping hundreds of files next to the archive.
      const dir = await new Promise<string | null>((resolve) => {
        setDirInput(info.path.replace(/[^/\\]+$/, "") + baseName(info.path).replace(/\.[^.]+$/, "") + "/");
        setExtractTo({ resolve });
      });
      if (dir) await runExtract(dir, members);
      return;
    }
    await runExtract(info.path.replace(/[^/\\]+$/, ""), members);
  }, [info, runExtract, selected]);

  const pickDir = useCallback(async () => {
    if (!info) return;
    let dir: string | null = null;
    if (inTauri) {
      const { open } = await import("@tauri-apps/plugin-dialog");
      dir = await open({ directory: true, title: "解壓到哪個資料夾" });
    } else {
      dir = await new Promise<string | null>((resolve) => {
        setDirInput("D:/解壓到這裡/");
        setExtractTo({ resolve });
      });
    }
    if (dir) await runExtract(dir, selected);
  }, [info, runExtract, selected]);

  const copyPaths = async () => {
    if (!menu) return;
    try {
      await navigator.clipboard.writeText(menu.paths.join("\n"));
      setNotice(`已複製 ${menu.paths.length} 條內部路徑`);
    } catch {
      setNotice("複製失敗：瀏覽器不允許存取剪貼簿");
    }
    setMenu(null);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const typing = (e.target as HTMLElement | null)?.tagName === "INPUT" || (e.target as HTMLElement | null)?.tagName === "TEXTAREA";
      if (e.key === "F5") {
        e.preventDefault();
        if (current) void openPath(current.source);
      } else if (e.altKey && e.key === "ArrowUp") {
        e.preventDefault();
        jumpTo(levels.length - 2);
      } else if (!typing && e.key === "Delete") {
        setSelected([]);
      } else if (!typing && (e.ctrlKey || e.metaKey)) {
        const k = e.key.toLowerCase();
        if (k === "o") {
          e.preventDefault();
          void pickFile();
        } else if (k === "e" && !e.shiftKey) {
          e.preventDefault();
          void smartExtract();
        } else if (k === "e" && e.shiftKey) {
          e.preventDefault();
          void pickDir();
        } else if (k === "n") {
          e.preventDefault();
          setShowCompress(true);
        } else if (k === "a") {
          e.preventDefault();
          setSelected(sorted.map((s) => s.path));
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [levels, sorted, current, info]);

  const clickRow = (e: React.MouseEvent, path: string, index: number) => {
    if (e.shiftKey && lastIndex.current >= 0) {
      const [a, b] = [Math.min(lastIndex.current, index), Math.max(lastIndex.current, index)];
      setSelected(sorted.slice(a, b + 1).map((r) => r.path));
    } else if (e.ctrlKey || e.metaKey) {
      setSelected((s) => (s.includes(path) ? s.filter((x) => x !== path) : [...s, path]));
      lastIndex.current = index;
    } else {
      setSelected([path]);
      lastIndex.current = index;
    }
  };

  const suggestedArchive = useMemo(() => {
    if (!info) return "";
    const folder = current?.prefix.replace(/\/$/, "") ?? "";
    const stem = folder ? folder.split("/").slice(-1)[0] : baseName(info.path).replace(/\.[^.]+$/, "");
    return `${info.path.replace(/[^/\\]+$/, "")}${stem}.7z`;
  }, [current, info]);

  return (
    <Box sx={{ height: "100vh", display: "flex", flexDirection: "column" }}>
      <AppBar position="static" color="default" elevation={0} sx={{ borderBottom: 1, borderColor: "divider" }}>
        <Toolbar variant="dense" sx={{ gap: 1, flexWrap: "wrap" }}>
          <Tooltip title="開啟壓縮包 (Ctrl+O)">
            <Button size="small" startIcon={<FolderOutlined />} onClick={() => void pickFile()}>
              開啟
            </Button>
          </Tooltip>
          <Tooltip title="智慧解壓 (Ctrl+E)">
            <span>
              <Button size="small" variant="contained" startIcon={<DownloadOutlined />} disabled={!info} onClick={() => void smartExtract()}>
                智慧解壓{selected.length > 0 ? `（${selected.length} 項）` : ""}
              </Button>
            </span>
          </Tooltip>
          <Tooltip title="壓縮選取項目 (Ctrl+N)">
            <span>
              <Button size="small" startIcon={<UploadOutlined />} disabled={!info} onClick={() => setShowCompress(true)}>
                壓縮
              </Button>
            </span>
          </Tooltip>
          <Tooltip title="測試完整性，不寫檔">
            <span>
              <Button
                size="small"
                startIcon={<CheckCircleOutline />}
                disabled={!info}
                onClick={() => void runExtract(info?.path.replace(/[^/\\]+$/, "") ?? ".", selected, true)}
              >
                測試
              </Button>
            </span>
          </Tooltip>
          <Divider orientation="vertical" flexItem />
          <Tooltip title="上層 (Alt+↑)">
            <span>
              <IconButton size="small" aria-label="上層" disabled={levels.length <= 1} onClick={() => jumpTo(levels.length - 2)}>
                <ArrowUpwardOutlined fontSize="small" />
              </IconButton>
            </span>
          </Tooltip>
          <Box sx={{ flex: 1 }} />
          <TextField
            size="small"
            placeholder="搜尋目前資料夾"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            sx={{ width: 240 }}
            InputProps={{ startAdornment: <Box sx={{ mr: 1, display: "flex" }}><Search fontSize="small" sx={{ color: "text.secondary" }} /></Box> }}
          />
          <Chip size="small" label={engineChip} color="primary" variant="outlined" />
          <Tooltip title="解壓到指定目錄 (Ctrl+Shift+E)">
            <span>
              <Chip size="small" label={info ? `${info.format} · ${info.engine}` : "未開啟"} variant="outlined" onClick={() => void pickDir()} />
            </span>
          </Tooltip>
          <Tooltip title="重新讀取 (F5)">
            <IconButton size="small" aria-label="重新讀取" onClick={() => current && void openPath(current.source)}>
              <Refresh fontSize="small" />
            </IconButton>
          </Tooltip>
          <Tooltip title="設定">
            <IconButton size="small" aria-label="設定" onClick={() => setShowSettings(true)}>
              <SettingsOutlined fontSize="small" />
            </IconButton>
          </Tooltip>
        </Toolbar>
      </AppBar>

      <Box sx={{ px: 1.5, py: 0.75, display: "flex", alignItems: "center", gap: 1 }}>
        <ArchiveOutlined sx={{ fontSize: 18, color: "text.secondary" }} />
        {levels.length > 0 ? (
          <Breadcrumbs sx={{ "& .MuiLink-root": { cursor: "pointer" } }}>
            {levels.map((l, i) => (
              <Link
                key={`${l.source}-${l.prefix}-${i}`}
                underline={i === levels.length - 1 ? "none" : "hover"}
                color={i === levels.length - 1 ? "text.primary" : "primary.main"}
                onClick={() => jumpTo(i)}
              >
                {l.label}
              </Link>
            ))}
          </Breadcrumbs>
        ) : (
          <Typography variant="body2" color="text.secondary">
            把壓縮包或資料夾拖進來
          </Typography>
        )}
      </Box>

      <Box sx={{ flex: 1, mx: 1.5, mb: 1, overflow: "auto", border: 1, borderColor: "divider", borderRadius: 2, bgcolor: "background.paper" }}>
        {sorted.length === 0 ? (
          <Box sx={{ height: "100%", display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 1.5 }}>
            <ArchiveOutlined sx={{ fontSize: 48, color: "text.secondary" }} />
            <Typography variant="h6">{info ? (search ? "沒有符合的檔案" : "這個資料夾是空的") : "把壓縮包或資料夾拖進來"}</Typography>
            <Typography variant="body2" color="text.secondary">
              支援 7z / zip / tar / gz / bz2 / zst　中文舊編碼檔名自動修復　越界路徑預設阻擋
            </Typography>
          </Box>
        ) : (
          <Table size="small" stickyHeader sx={{ tableLayout: "fixed" }}>
            <TableHead>
              <TableRow>
                {COLUMNS.map((c) => (
                  <TableCell key={c.label} align={c.align} sx={{ width: c.width || undefined }} onClick={() => setSort((s) => (s.key === c.key ? { key: c.key, desc: !s.desc } : { key: c.key, desc: false }))}>
                    {c.label}
                    {sort.key === c.key ? (sort.desc ? " ▼" : " ▲") : ""}
                  </TableCell>
                ))}
                <TableCell align="center">檔名編碼</TableCell>
              </TableRow>
            </TableHead>
            <TableBody>
              {sorted.map((e, i) => (
                <TableRow
                  key={e.path}
                  className={e.unsafePath ? "unsafe" : undefined}
                  selected={selected.includes(e.path)}
                  hover
                  onClick={(ev) => clickRow(ev, e.path, i)}
                  onDoubleClick={() => void enter(e)}
                  onContextMenu={(ev) => {
                    ev.preventDefault();
                    if (!selected.includes(e.path)) setSelected([e.path]);
                    setMenu({ anchor: ev.currentTarget, paths: selected.includes(e.path) ? selected : [e.path] });
                  }}
                >
                  <TableCell>
                    <Box sx={{ display: "flex", alignItems: "center", gap: 1, minWidth: 0 }}>
                      {childIcon(e)}
                      <Box component="span" sx={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                        {e.name}
                      </Box>
                    </Box>
                  </TableCell>
                  <TableCell align="right">{humanSize(e.size)}</TableCell>
                  <TableCell align="right">{e.packed === null ? "—" : humanSize(e.packed)}</TableCell>
                  <TableCell align="right">{humanRatio(e.size, e.packed)}</TableCell>
                  <TableCell>{humanTime(e.mtime)}</TableCell>
                  <TableCell align="center">{e.attrs}</TableCell>
                  <TableCell align="center">{e.codec ? <Chip size="small" label={e.codec} /> : ""}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Box>

      <JobsPanel
        jobs={jobs}
        onCancel={(id) => void api.cancel(id)}
        onClear={() => {
          void api.clearJobs().then(() => void poll());
        }}
      />

      <Box sx={{ px: 2, py: 0.75, borderTop: 1, borderColor: "divider", display: "flex", gap: 2, alignItems: "center" }}>
        <Typography variant="caption" color="text.secondary">
          {info
            ? `${current?.prefix ?? ""}${sorted.length} 項 · 包內共 ${info.entries.length} 條 · ${humanSize(info.totalSize)} · ${info.engine}`
            : "待機"}
        </Typography>
        <Box sx={{ flex: 1 }} />
        {selected.length > 0 && (
          <Typography variant="caption" color="text.secondary">
            已選 {selected.length} 項
          </Typography>
        )}
        <Typography variant="caption" color="text.secondary">
          {inTauri ? "Tauri" : "瀏覽器預覽"}
        </Typography>
      </Box>

      <Menu open={!!menu} anchorEl={menu?.anchor ?? null} onClose={() => setMenu(null)}>
        <MenuItem
          onClick={() => {
            const path = menu?.paths[0];
            const row = view.find((e) => e.path === path);
            setMenu(null);
            if (row) void enter(row);
          }}
        >
          開啟 / 進入
        </MenuItem>
        <MenuItem
          onClick={() => {
            const paths = menu?.paths ?? [];
            setMenu(null);
            void runExtract(info?.path.replace(/[^/\\]+$/, "") ?? ".", paths);
          }}
        >
          解壓選取項目到壓縮包所在資料夾
        </MenuItem>
        <Divider />
        <MenuItem onClick={() => void copyPaths()}>複製完整路徑（{menu?.paths.length ?? 0} 條）</MenuItem>
        <MenuItem
          onClick={() => {
            const paths = menu?.paths ?? [];
            setMenu(null);
            void api
              .hash(paths[0] ?? "", "sha256")
              .then((h) => setNotice(`SHA-256：${h}`))
              .catch((e) => setNotice(`雜湊失敗：${e}`));
          }}
        >
          計算 SHA-256
        </MenuItem>
      </Menu>

      <CompressDialog
        open={showCompress}
        onClose={() => setShowCompress(false)}
        settings={settings}
        suggestedArchive={suggestedArchive}
        initialSources={selected.length > 0 ? selected : current?.prefix ? [current.prefix] : []}
        onStart={(opts) => {
          setShowCompress(false);
          void api
            .startCompress(opts)
            .then(() => poll())
            .then(() => setNotice("已送進工作佇列：壓縮"))
            .catch((e) => setNotice(`無法開始工作：${e}`));
        }}
      />

      <SettingsDialog
        open={showSettings}
        onClose={() => setShowSettings(false)}
        settings={settings}
        onSaved={(next) => {
          setSettings(next);
          setNotice("設定已儲存");
        }}
      />

      <Dialog open={!!askPw} onClose={() => { askPw?.resolve(null); setAskPw(null); }}>
        <DialogTitle>這個壓縮包需要密碼</DialogTitle>
        <DialogContent>
          <TextField
            autoFocus
            fullWidth
            type="password"
            label="密碼"
            value={pwInput}
            onChange={(e) => setPwInput(e.target.value)}
            helperText="密碼只傳給引擎，不會寫進記錄或畫面輸出"
          />
        </DialogContent>
        <DialogActions>
          <Button onClick={() => { askPw?.resolve(null); setAskPw(null); setPwInput(""); }}>取消</Button>
          <Button variant="contained" onClick={() => { askPw?.resolve(pwInput); setAskPw(null); setPwInput(""); }}>確定</Button>
        </DialogActions>
      </Dialog>

      <Dialog open={!!extractTo} onClose={() => { extractTo?.resolve(null); setExtractTo(null); }}>
        <DialogTitle>解壓到指定目錄</DialogTitle>
        <DialogContent>
          <TextField
            autoFocus
            fullWidth
            label="目標資料夾"
            value={dirInput}
            onChange={(e) => setDirInput(e.target.value)}
            helperText={inTauri ? "建議直接用工具列的資料夾選取器" : "瀏覽器預覽只能手打路徑"}
          />
        </DialogContent>
        <DialogActions>
          <Button onClick={() => { extractTo?.resolve(null); setExtractTo(null); }}>取消</Button>
          <Button variant="contained" disabled={!dirInput.trim()} onClick={() => { extractTo?.resolve(dirInput.trim()); setExtractTo(null); }}>
            解壓
          </Button>
        </DialogActions>
      </Dialog>

      <Snackbar open={!!notice} autoHideDuration={6000} onClose={() => setNotice(null)} message={notice ?? ""} />
    </Box>
  );
}
