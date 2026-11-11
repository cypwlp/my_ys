import React from "react";
import ReactDOM from "react-dom/client";
import { CssBaseline, ThemeProvider } from "@mui/material";
import App from "./App";
import { buildTheme } from "./theme";

// Both switches are read from the URL so the same bundle can be eyeballed in four
// combinations without rebuilding: ?mode=light&skin=fluent
const params = new URLSearchParams(window.location.search);
const mode = params.get("mode") === "light" ? "light" : "dark";
const skin = params.get("skin") === "fluent" ? "fluent" : "material";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ThemeProvider theme={buildTheme(mode, skin)}>
      <CssBaseline />
      <App />
    </ThemeProvider>
  </React.StrictMode>,
);
