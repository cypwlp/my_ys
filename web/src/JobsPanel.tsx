import { Box, Button, Chip, IconButton, LinearProgress, Tooltip, Typography } from "@mui/material";
import CloseIcon from "@mui/icons-material/Close";
import DeleteSweepOutlinedIcon from "@mui/icons-material/DeleteSweepOutlined";
import type { Job } from "./types";
import { humanSize } from "./format";

const STATE_LABEL: Record<Job["state"], { text: string; color: "primary" | "success" | "error" | "default" }> = {
  Running: { text: "進行中", color: "primary" },
  Done: { text: "完成", color: "success" },
  Failed: { text: "失敗", color: "error" },
  Cancelled: { text: "已取消", color: "default" },
};

const KIND_LABEL: Record<string, string> = { extract: "解壓", compress: "壓縮", test: "測試" };

export default function JobsPanel({
  jobs,
  onCancel,
  onClear,
}: {
  jobs: Job[];
  onCancel: (id: number) => void;
  onClear: () => void;
}) {
  if (jobs.length === 0) return null;
  const running = jobs.filter((j) => j.state === "Running").length;

  return (
    <Box sx={{ mx: 1.5, mb: 1, p: 1.25, border: 1, borderColor: "divider", borderRadius: 2, bgcolor: "background.paper" }}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1, mb: 1 }}>
        <Typography variant="subtitle2">工作佇列</Typography>
        <Typography variant="caption" color="text.secondary">
          {running > 0 ? `${running} 個進行中 · 共 ${jobs.length} 個` : `共 ${jobs.length} 個`}
        </Typography>
        <Box sx={{ flex: 1 }} />
        {running > 0 && (
          <Button
            size="small"
            color="error"
            onClick={() => jobs.filter((j) => j.state === "Running").forEach((j) => onCancel(j.id))}
          >
            全部取消
          </Button>
        )}
        <Tooltip title="清除已完成的工作">
          <span>
            <IconButton size="small" disabled={running === jobs.length} onClick={onClear}>
              <DeleteSweepOutlinedIcon fontSize="small" />
            </IconButton>
          </span>
        </Tooltip>
      </Box>

      <Box sx={{ display: "flex", flexDirection: "column", gap: 1, maxHeight: 190, overflow: "auto" }}>
        {jobs.map((j) => {
          const s = STATE_LABEL[j.state];
          return (
            <Box key={j.id} sx={{ display: "flex", alignItems: "center", gap: 1.25 }}>
              <Chip size="small" label={s.text} color={s.color} sx={{ width: 68 }} />
              <Box sx={{ flex: 1, minWidth: 0 }}>
                <Box sx={{ display: "flex", gap: 1, alignItems: "baseline" }}>
                  <Typography variant="body2" noWrap sx={{ fontWeight: 600 }}>
                    {KIND_LABEL[j.kind] ?? j.kind} · {j.label}
                  </Typography>
                  <Box sx={{ flex: 1 }} />
                  {j.state === "Done" && (
                    <Typography variant="caption" color="text.secondary">
                      {j.items} 項 · {humanSize(j.bytes)}
                      {j.blocked > 0 ? ` · 阻擋 ${j.blocked}` : ""}
                    </Typography>
                  )}
                </Box>
                <LinearProgress
                  variant={j.percent >= 0 ? "determinate" : "indeterminate"}
                  value={Math.max(0, j.percent)}
                  color={j.state === "Failed" ? "error" : "primary"}
                  sx={{ height: 5, mt: 0.5, borderRadius: 1 }}
                />
                <Typography variant="caption" color="text.secondary" noWrap sx={{ display: "block" }}>
                  {j.error ?? j.detail}
                </Typography>
              </Box>
              <IconButton
                size="small"
                disabled={j.state !== "Running"}
                onClick={() => onCancel(j.id)}
                aria-label="取消這個工作"
              >
                <CloseIcon fontSize="small" />
              </IconButton>
            </Box>
          );
        })}
      </Box>
    </Box>
  );
}
