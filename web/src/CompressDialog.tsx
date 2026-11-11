import { useEffect, useState } from "react";
import {
  Box,
  Button,
  Checkbox,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  FormControlLabel,
  MenuItem,
  Slider,
  Stack,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import { PlaylistAdd, DeleteOutline } from "@mui/icons-material";
import { inTauri } from "./api";
import type { CompressOptions, Format, Settings } from "./types";

const FORMATS: { value: Format; label: string; writable: boolean }[] = [
  { value: "SevenZip", label: "7z（LZMA2，最高壓縮比）", writable: true },
  { value: "Zip", label: "ZIP（相容性最好）", writable: true },
  { value: "TarGz", label: "tar.gz（Unix 常見）", writable: true },
  { value: "TarBz2", label: "tar.bz2（比 gz 小、比 gz 慢）", writable: true },
  { value: "Tar", label: "tar（不壓縮，只打包）", writable: true },
  // Read-only here: the pure-Rust decoders exist but no encoder, and this machine
  // has no C compiler for zstd/xz2. Listing them as creatable would be a lie.
  { value: "TarXz", label: "tar.xz（僅能讀取）", writable: false },
  { value: "TarZst", label: "tar.zst（僅能讀取）", writable: false },
  { value: "Rar", label: "RAR（僅能讀取，需完整 7-Zip）", writable: false },
];

export default function CompressDialog({
  open,
  onClose,
  settings,
  suggestedArchive,
  initialSources,
  onStart,
}: {
  open: boolean;
  onClose: () => void;
  settings: Settings;
  suggestedArchive: string;
  initialSources: string[];
  onStart: (opts: CompressOptions) => void;
}) {
  const [archive, setArchive] = useState(suggestedArchive);
  const [format, setFormat] = useState<Format>(settings.defaultFormat);
  const [level, setLevel] = useState(settings.level);
  const [threads, setThreads] = useState(settings.threads);
  const [password, setPassword] = useState("");
  const [header, setHeader] = useState(false);
  const [sources, setSources] = useState<string[]>(initialSources);

  // Re-open with the current selection instead of whatever the last time left behind.
  useEffect(() => {
    if (open) {
      setArchive(suggestedArchive);
      setSources(initialSources);
      setFormat(settings.defaultFormat);
      setLevel(settings.level);
      setThreads(settings.threads);
      setPassword("");
      setHeader(false);
    }
  }, [open, suggestedArchive, initialSources, settings]);

  const addSources = async () => {
    if (!inTauri) {
      setSources((s) => [...s, "D:/demo/新增資料夾"]);
      return;
    }
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: true, title: "選擇要壓縮的檔案或資料夾" });
    if (!picked) return;
    const list = Array.isArray(picked) ? picked : [picked];
    setSources((s) => [...new Set([...s, ...list])]);
  };

  const writable = FORMATS.filter((f) => f.writable);
  const invalid = sources.length === 0 || !archive.trim();

  return (
    <Dialog open={open} onClose={onClose} fullWidth maxWidth="xs"
        PaperProps={{ sx: { maxHeight: "88vh" } }}
      >
      <DialogTitle>壓縮</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          <TextField
            label="壓縮包名稱"
            value={archive}
            onChange={(e) => setArchive(e.target.value)}
            helperText="含完整路徑；副檔名會決定預設格式"
          />

          <Box>
            <Typography variant="body2" color="text.secondary" sx={{ mb: 0.5 }}>
              來源（{sources.length} 項）
            </Typography>
            <Box sx={{ border: 1, borderColor: "divider", borderRadius: 1, p: 1, maxHeight: 150, overflow: "auto" }}>
              {sources.length === 0 ? (
                <Typography variant="body2" color="text.secondary">
                  尚未選擇來源
                </Typography>
              ) : (
                sources.map((s) => (
                  <Box key={s} sx={{ display: "flex", alignItems: "center", gap: 1 }}>
                    <Checkbox size="small" checked onChange={() => setSources((list) => list.filter((x) => x !== s))} />
                    <Typography variant="body2" noWrap sx={{ flex: 1 }}>
                      {s}
                    </Typography>
                  </Box>
                ))
              )}
            </Box>
            <Button size="small" startIcon={<PlaylistAdd />} onClick={() => void addSources()} sx={{ mt: 0.75 }}>
              加入檔案或資料夾
            </Button>
          </Box>

          <TextField
            select
            label="格式"
            value={format}
            onChange={(e) => setFormat(e.target.value as Format)}
            helperText="標示「僅能讀取」的格式沒有純 Rust 編碼器；這台機器也沒有 C 編譯器可以補上"
          >
            {writable.map((f) => (
              <MenuItem key={f.value} value={f.value}>
                {f.label}
              </MenuItem>
            ))}
          </TextField>

          <Box sx={{ px: 1 }}>
            <Typography variant="body2" color="text.secondary">
              壓縮等級：{level}（{level <= 3 ? "快" : level <= 6 ? "平衡" : "最小體積"}）
            </Typography>
            <Slider value={level} min={1} max={9} step={1} marks onChange={(_, v) => setLevel(v as number)} />
          </Box>

          <TextField
            select
            label="執行緒"
            value={threads}
            onChange={(e) => setThreads(Number(e.target.value))}
            helperText="0 表示交給引擎決定"
            sx={{ mt: 1 }}
          >
            {[0, 2, 4, 8, 16].map((n) => (
              <MenuItem key={n} value={n}>
                {n === 0 ? "自動" : `${n} 緒`}
              </MenuItem>
            ))}
          </TextField>

          <TextField
            type="password"
            label="密碼（留空＝不加密）"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            helperText="7-Zip 只接受命令列密碼，因此處理期間會短暫出現在程序清單；這是 7-Zip 自身的限制"
          />
          <Box>
            <FormControlLabel
              control={<Checkbox checked={header} disabled={!password} onChange={(e) => setHeader(e.target.checked)} />}
              label="同時加密檔頭（連檔案清單都要密碼才能看）"
            />
          </Box>

          <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
            <DeleteOutline fontSize="small" sx={{ color: "text.secondary" }} />
            <Typography variant="caption" color="text.secondary">
              壓縮完成後不會自動刪除來源
            </Typography>
          </Box>
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose}>取消</Button>
        <Tooltip title={invalid ? "先選來源並填入名稱" : "送進工作佇列"}>
          <span>
            <Button
              variant="contained"
              disabled={invalid}
              onClick={() =>
                onStart({
                  archive: archive.trim(),
                  sources,
                  format,
                  password: password || undefined,
                  headerEncryption: header && !!password,
                  level,
                  threads,
                })
              }
            >
              開始壓縮
            </Button>
          </span>
        </Tooltip>
      </DialogActions>
    </Dialog>
  );
}
