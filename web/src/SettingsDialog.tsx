import { useEffect, useState } from "react";
import {
  Box,
  Button,
  Chip,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Divider,
  FormControlLabel,
  MenuItem,
  Radio,
  RadioGroup,
  Slider,
  Stack,
  Tab,
  Tabs,
  TextField,
  Typography,
} from "@mui/material";
import Checkbox from "@mui/material/Checkbox";
import HistoryToggleOffOutlinedIcon from "@mui/icons-material/HistoryToggleOffOutlined";
import { api } from "./api";
import type { Format, Overwrite, Settings } from "./types";

const FORMATS: { value: Format; label: string }[] = [
  { value: "SevenZip", label: "7z" },
  { value: "Zip", label: "ZIP" },
  { value: "TarGz", label: "tar.gz" },
  { value: "Tar", label: "tar" },
];

const OVERWRITE: { value: Overwrite; label: string; hint: string }[] = [
  { value: "KeepBoth", label: "改名保留兩者", hint: "已存在時自動加上 (2)、(3)…" },
  { value: "Ask", label: "每次都問", hint: "目前由引擎以預設處理" },
  { value: "Always", label: "直接覆蓋", hint: "不提示，舊檔消失" },
  { value: "Skip", label: "跳過已存在", hint: "舊檔保持原樣" },
  { value: "Newer", label: "只覆蓋較舊的", hint: "依修改時間判斷" },
];

export default function SettingsDialog({
  open,
  onClose,
  settings,
  onSaved,
}: {
  open: boolean;
  onClose: () => void;
  settings: Settings;
  onSaved: (next: Settings) => void;
}) {
  const [tab, setTab] = useState(0);
  const [draft, setDraft] = useState<Settings>(settings);
  const [engineRows, setEngineRows] = useState<{ id: string; kind: string; formats: [string, boolean][]; path: string | null }[]>([]);
  const [regText, setRegText] = useState("");
  const [regInstalled, setRegInstalled] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setDraft(settings);
    setError(null);
    void api.engines().then(setEngineRows);
    if (tab === 2) {
      void api.shellPreview().then(setRegText);
      void api.shellInstalled().then(setRegInstalled);
    }
  }, [open, settings, tab]);

  const patch = (part: Partial<Settings>) => setDraft((d) => ({ ...d, ...part }));

  const save = async () => {
    setSaving(true);
    try {
      await api.saveSettings(draft);
      onSaved(draft);
      onClose();
    } catch (e) {
      // A rejected save must stay visible: silently closing would lose the edit.
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onClose={onClose} fullWidth maxWidth="md">
      <DialogTitle>設定</DialogTitle>
      <Tabs value={tab} onChange={(_, v) => setTab(v)} sx={{ px: 3, borderBottom: 1, borderColor: "divider" }}>
        <Tab label="一般" />
        <Tab label="引擎" />
        <Tab label="檔案總管" />
      </Tabs>
      <DialogContent sx={{ pt: "16px !important" }}>
        {tab === 0 && (
          <Stack spacing={2.5}>
            <Box>
              <Typography variant="body2" color="text.secondary" sx={{ mb: 0.5 }}>
                配色
              </Typography>
              <RadioGroup row value={draft.theme} onChange={(e) => patch({ theme: e.target.value as Settings["theme"] })}>
                <FormControlLabel value="system" control={<Radio size="small" />} label="跟隨系統" />
                <FormControlLabel value="dark" control={<Radio size="small" />} label="深色" />
                <FormControlLabel value="light" control={<Radio size="small" />} label="淺色" />
              </RadioGroup>
            </Box>
            <Box>
              <Typography variant="body2" color="text.secondary" sx={{ mb: 0.5 }}>
                外观风格
              </Typography>
              <RadioGroup row value={draft.skin} onChange={(e) => patch({ skin: e.target.value as Settings["skin"] })}>
                <FormControlLabel value="material" control={<Radio size="small" />} label="Material 3" />
                <FormControlLabel value="fluent" control={<Radio size="small" />} label="Fluent（WinUI）" />
              </RadioGroup>
            </Box>

            <Box>
              <Typography variant="body2" color="text.secondary" sx={{ mb: 0.5 }}>
                解壓時遇到同名檔案
              </Typography>
              <RadioGroup value={draft.overwrite} onChange={(e) => patch({ overwrite: e.target.value as Overwrite })}>
                {OVERWRITE.map((o) => (
                  <FormControlLabel
                    key={o.value}
                    value={o.value}
                    control={<Radio size="small" />}
                    label={
                      <Box component="span">
                        {o.label}
                        <Typography variant="caption" color="text.secondary" sx={{ ml: 1 }}>
                          {o.hint}
                        </Typography>
                      </Box>
                    }
                  />
                ))}
              </RadioGroup>
            </Box>

            <Box sx={{ px: 1 }}>
              <Typography variant="body2" color="text.secondary">
                預設壓縮等級：{draft.level}
              </Typography>
              <Slider value={draft.level} min={1} max={9} step={1} marks onChange={(_, v) => patch({ level: v as number })} />
            </Box>

            <TextField
              select
              label="預設格式"
              value={draft.defaultFormat}
              onChange={(e) => patch({ defaultFormat: e.target.value as Format })}
              sx={{ mt: 1 }}
            >
              {FORMATS.map((f) => (
                <MenuItem key={f.value} value={f.value}>
                  {f.label}
                </MenuItem>
              ))}
            </TextField>

            <Box>
              <FormControlLabel
                control={<Checkbox size="small" checked={draft.repairNames} onChange={(e) => patch({ repairNames: e.target.checked })} />}
                label={
                  <Box component="span">
                    自動修復舊編碼檔名
                    <Typography variant="caption" color="text.secondary" sx={{ ml: 1 }}>
                      把 GB18030／Big5 存成亂碼的檔名改回來，並在「檔名編碼」欄標示來源
                    </Typography>
                  </Box>
                }
              />
              <FormControlLabel
                control={<Checkbox size="small" checked={draft.useRecycleBin} onChange={(e) => patch({ useRecycleBin: e.target.checked })} />}
                label="刪除時先進資源回收桶"
              />
              <FormControlLabel
                control={<Checkbox size="small" checked={draft.openAfterExtract} onChange={(e) => patch({ openAfterExtract: e.target.checked })} />}
                label="解壓完成後開啟目標資料夾"
              />
            </Box>

            <Divider />
            <Box>
              <Box sx={{ display: "flex", alignItems: "center", gap: 1, mb: 0.5 }}>
                <Typography variant="body2" color="text.secondary">
                  近期使用（{draft.history.length}）
                </Typography>
                <Box sx={{ flex: 1 }} />
                <Button
                  size="small"
                  startIcon={<HistoryToggleOffOutlinedIcon />}
                  disabled={draft.history.length === 0}
                  onClick={() => {
                    patch({ history: [] });
                    void api.clearHistory();
                  }}
                >
                  清除
                </Button>
              </Box>
              <Box sx={{ maxHeight: 140, overflow: "auto", border: 1, borderColor: "divider", borderRadius: 1, p: 1 }}>
                {draft.history.length === 0 ? (
                  <Typography variant="caption" color="text.secondary">
                    還沒有紀錄
                  </Typography>
                ) : (
                  draft.history.map((h) => (
                    <Typography key={h} variant="body2" noWrap>
                      {h}
                    </Typography>
                  ))
                )}
              </Box>
            </Box>
          </Stack>
        )}

        {tab === 1 && (
          <Stack spacing={1.5}>
            <Typography variant="body2" color="text.secondary">
              引擎依格式優先順序嘗試；只有「做不到」才會換下一家，密碼錯誤與損毀不會被吞掉。
            </Typography>
            {engineRows.map((row) => (
              <Box key={row.id} sx={{ border: 1, borderColor: "divider", borderRadius: 1, p: 1.25 }}>
                <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
                  <Typography sx={{ fontWeight: 600 }}>{row.id}</Typography>
                  <Chip size="small" label={row.kind} variant="outlined" />
                  <Box sx={{ flex: 1 }} />
                  <Typography variant="caption" color="text.secondary" noWrap>
                    {row.path ?? "內建（無外部程式）"}
                  </Typography>
                </Box>
                <Box sx={{ display: "flex", gap: 0.5, flexWrap: "wrap", mt: 0.75 }}>
                  {row.formats.map(([label, canWrite]) => (
                    <Chip
                      key={label}
                      size="small"
                      label={canWrite ? `${label} 讀寫` : `${label} 僅讀`}
                      color={canWrite ? "primary" : "default"}
                      variant="outlined"
                    />
                  ))}
                </Box>
              </Box>
            ))}
            <Typography variant="caption" color="text.secondary">
              沒有 7zip 這列時，RAR／xz／zst 與加密 ZIP 都無法處理。
            </Typography>
          </Stack>
        )}

        {tab === 2 && (
          <Stack spacing={1.5}>
            <Chip
              size="small"
              label={regInstalled ? "已寫入（HKCU）" : "尚未安裝"}
              color={regInstalled ? "success" : "default"}
              sx={{ alignSelf: "flex-start" }}
            />
            <Typography variant="body2" color="text.secondary">
              只寫 HKCU、動詞前綴 QuickArc.，不碰其他程式的鍵。下面是要寫入的內容，尚未套用。
            </Typography>
            <Box
              component="pre"
              sx={{
                m: 0,
                p: 1.25,
                border: 1,
                borderColor: "divider",
                borderRadius: 1,
                bgcolor: "background.default",
                maxHeight: 260,
                overflow: "auto",
                fontSize: 12,
                whiteSpace: "pre-wrap",
                userSelect: "text",
              }}
            >
              {regText || "; 載入中…"}
            </Box>
            <Button
              size="small"
              onClick={() => {
                const blob = new Blob([regText], { type: "text/plain" });
                const a = document.createElement("a");
                a.href = URL.createObjectURL(blob);
                a.download = "quickarc-shell.reg";
                a.click();
                URL.revokeObjectURL(a.href);
              }}
              disabled={!regText}
            >
              下載這個 .reg 檔（自己看过再决定是否双击）
            </Button>
          </Stack>
        )}

        {error && (
          <Typography color="error" variant="body2" sx={{ mt: 1.5 }}>
            儲存失敗：{error}
          </Typography>
        )}
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose}>取消</Button>
        <Button variant="contained" onClick={() => void save()} disabled={saving}>
          {saving ? "儲存中…" : "儲存"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}
