import { createTheme } from "@mui/material/styles";

/**
 * One token set for both skins.  The names match Palette.*.axaml in the Avalonia
 * build so a colour decision can be compared file to file instead of by eye.
 */
const tokens = {
  light: {
    appBg: "#F4F5F7",
    cardBg: "#FFFFFF",
    headerBg: "#F0F1F4",
    text: "#1B1C1E",
    text2: "#5A6069",
    stroke: "#DDE1E6",
    accent: "#6750A4",
    accentText: "#FFFFFF",
    accentSoft: "#EDE7F6",
    selected: "#E5DEF6",
    folder: "#B26A00",
    archive: "#6B5BD7",
    danger: "#C0392B",
  },
  dark: {
    appBg: "#16171A",
    cardBg: "#1E2023",
    headerBg: "#232629",
    text: "#E8EAED",
    text2: "#9AA1AC",
    stroke: "#33373C",
    accent: "#B79CFF",
    accentText: "#211A31",
    accentSoft: "#2A2440",
    selected: "#3A3155",
    folder: "#F6B26B",
    archive: "#8B7CF6",
    danger: "#E06C75",
  },
} as const;

export function buildTheme(mode: "light" | "dark", shape: "material" | "fluent") {
  const c = tokens[mode];
  return createTheme({
    palette: {
      mode,
      primary: { main: c.accent, contrastText: c.accentText },
      background: { default: c.appBg, paper: c.cardBg },
      text: { primary: c.text, secondary: c.text2 },
      divider: c.stroke,
      error: { main: c.danger },
    },
    // M3 base is 12; the Fluent skin wants the squarer WinUI radius.
    shape: { borderRadius: shape === "material" ? 12 : 6 },
    typography: {
      fontFamily:
        'Inter, "Microsoft JhengHei UI", "Segoe UI Variable Text", "PingFang SC", sans-serif',
      // M3 labels read smaller than Material UI's defaults; the header row is the
      // one place that has to line up with numeric cells, so it is set explicitly.
      fontSize: 14,
      button: { textTransform: "none", fontWeight: 600 },
    },
    components: {
      MuiPaper: { styleOverrides: { root: { backgroundImage: "none" } } },
      MuiButton: {
        defaultProps: { disableElevation: true },
        styleOverrides: {
          root: { borderRadius: 999, textTransform: "none" as const },
          containedPrimary: { backgroundColor: c.accent, color: c.accentText },
        },
      },
      MuiIconButton: { styleOverrides: { root: { borderRadius: 999 } } },
      MuiChip: { styleOverrides: { root: { borderRadius: 8, fontWeight: 600 } } },
      MuiTableCell: {
        styleOverrides: {
          root: { borderBottom: `1px solid ${c.stroke}`, padding: "8px 10px" },
          head: {
            background: c.headerBg,
            color: c.text2,
            fontWeight: 600,
            fontSize: 12,
            cursor: "pointer",
          },
        },
      },
      MuiTableRow: {
        styleOverrides: {
          root: {
            "&.unsafe": { color: c.danger },
            "&.selected": { background: c.selected },
          },
        },
      },
    },
  });
}

export const colors = tokens;
