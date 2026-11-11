// Drive the UI over CDP and assert the DOM, instead of clicking screen coordinates.
//
// Synthetic mouse events need the window to be genuinely foreground, and Windows
// refuses SetForegroundWindow to a background process, so a coordinate click is not
// a reliable test. Node 22+ has a global WebSocket, so this needs no new packages.
//
//   node tools/ui_cdp.mjs [url]            default: http://localhost:1431/?mode=dark
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";

const EDGE = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const PORT = 9333;
const url = process.argv[2] ?? "http://localhost:1431/?mode=dark";
const out = process.argv[3] ?? ".shots/cdp-result.png";

const edge = spawn(EDGE, [
  "--headless=new",
  `--remote-debugging-port=${PORT}`,
  "--disable-gpu",
  "--no-first-run",
  "--window-size=1440,900",
  "about:blank",
], { stdio: "ignore" });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function target() {
  for (let i = 0; i < 60; i += 1) {
    try {
      const res = await fetch(`http://127.0.0.1:${PORT}/json/list`);
      const list = await res.json();
      const page = list.find((t) => t.type === "page");
      if (page?.webSocketDebuggerUrl) return page.webSocketDebuggerUrl;
    } catch {
      /* not up yet */
    }
    await sleep(250);
  }
  throw new Error("no CDP target");
}

let id = 0;
const pending = new Map();
let ws;

function send(method, params = {}) {
  return new Promise((resolve, reject) => {
    const msgId = ++id;
    pending.set(msgId, { resolve, reject });
    ws.send(JSON.stringify({ id: msgId, method, params }));
  });
}

async function evaluate(expression) {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.text + " " + JSON.stringify(r.result));
  return r.result.value;
}

const wsUrl = await target();
ws = new WebSocket(wsUrl);
await new Promise((resolve, reject) => {
  ws.addEventListener("open", resolve);
  ws.addEventListener("error", reject);
});
ws.addEventListener("message", (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
  }
});

await send("Page.enable");
await send("Runtime.enable");
await send("Page.navigate", { url });
await sleep(2500);

/** React 19 delegates listeners at the root, so element.click() does not reach the
 *  handler; a bubbling MouseEvent does.  Every click in this harness goes through it. */
const clickByLabel = (text) => `
  (() => {
    const b = [...document.querySelectorAll('button')].find(x => x.textContent.trim().startsWith(${JSON.stringify(text)}));
    if (!b) return "no-button";
    b.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }));
    return "clicked";
  })()
`;
const clickByAria = (label) => {
  const selector = `button[aria-label="${label}"]`;
  return `
  (() => {
    const b = document.querySelector(${JSON.stringify(selector)});
    if (!b) return "no-button";
    b.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }));
    return "clicked";
  })()`;
};
const dialogButton = (text) => `
  (() => {
    const b = [...document.querySelectorAll('.MuiDialog-root button')].find(x => x.textContent.trim() === ${JSON.stringify(text)});
    if (!b) return "no-button";
    b.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }));
    return "clicked";
  })()
`;

const results = [];
const check = (name, ok, detail) => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  -- " + detail : ""}`);
};

// --- the toolbar buttons exist and are wired -------------------------------
const buttonLabels = await evaluate(`
  [...document.querySelectorAll('button')].map(b => b.textContent.trim()).filter(Boolean)
`);
check("工具列按鈕都在", ["開啟", "壓縮", "測試"].every((t) => buttonLabels.some((b) => b.startsWith(t))),
  buttonLabels.slice(0, 6).join(" | "));

// --- clicking 測試 must create a job row in the panel ----------------------
await evaluate(clickByLabel("測試"));
await sleep(1200);
const afterTest = await evaluate(`
  (() => {
    const text = document.body.innerText;
    return {
      panelVisible: text.includes('工作佇列'),
      percentBar: !!document.querySelector('.MuiLinearProgress-root'),
      running: text.includes('進行中'),
      snackbar: document.querySelector('.MuiSnackbar-root')?.innerText ?? "",
    };
  })()
`);
check("測試按钮建立了工作", afterTest.panelVisible && afterTest.percentBar && afterTest.running,
  JSON.stringify(afterTest));

// --- cancel must move it out of Running -----------------------------------
await evaluate(clickByAria("取消這個工作"));
await sleep(1000);
const cancelled = await evaluate(`
  (() => {
    const chips = [...document.querySelectorAll('.MuiChip-label')].map((c) => c.textContent.trim());
    return { chips, stillRunning: document.body.innerText.includes('進行中') };
  })()
`);
// The job's own chip must read 已取消; a row that says both 完成 and 已取消 is a lie.
check(
  "取消按钮把工作改成已取消",
  cancelled.chips.includes("已取消") && !cancelled.chips.includes("完成") && !cancelled.stillRunning,
  JSON.stringify(cancelled),
);
// --- the compress dialog opens, validates, and submits --------------------
await evaluate(clickByLabel("壓縮"));
await sleep(700);
const dialog = await evaluate(`
  (() => {
    const d = document.querySelector('.MuiDialog-root');
    if (!d) return { open: false };
    const start = [...d.querySelectorAll('button')].find(b => b.textContent.includes('開始壓縮'));
    return {
      open: true,
      hasArchiveField: !!d.querySelector('input#archive, .MuiInputLabel-root'),
      startDisabled: start ? start.disabled : null,
      noteVisible: d.innerText.includes('壓縮完成後不會自動刪除來源'),
      // the footer must not be painted over the action row
      actionsBelowContent: (() => {
        const actions = d.querySelector('.MuiDialogActions-root')?.getBoundingClientRect();
        const content = d.querySelector('.MuiDialogContent-root')?.getBoundingClientRect();
        return actions && content ? actions.top >= content.bottom - 1 : null;
      })(),
    };
  })()
`);
check("壓縮對話框開啟", dialog.open === true, JSON.stringify(dialog));
check("沒有來源時「開始壓縮」是禁用", dialog.startDisabled === true);
check("對話框說明不再疊在按鈕上", dialog.actionsBelowContent === true, String(dialog.actionsBelowContent));

// --- settings dialog ------------------------------------------------------
await evaluate(dialogButton("取消"));
await sleep(600);
await evaluate(clickByAria("設定"));
await sleep(900);
const settings = await evaluate(`
  (() => {
    const tabs = [...document.querySelectorAll('.MuiTab-root')].map(t => t.textContent.trim());
    return { tabs, open: !!document.querySelector('.MuiDialog-root') };
  })()
`);
check("設定對話框有三個分頁", settings.open && settings.tabs.length === 3, JSON.stringify(settings));

await evaluate(`
  (() => {
    const tab = [...document.querySelectorAll('.MuiTab-root')][1];
    tab?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }));
    return !!tab;
  })()
`);
await sleep(500);
const enginesTab = await evaluate(`
  (() => {
    const dialogs = [...document.querySelectorAll('.MuiDialogContent-root')];
    return (dialogs[dialogs.length - 1]?.innerText ?? '').slice(0, 240);
  })()
`);
check("引擎分頁列出引擎", /tar-managed|zip-managed/.test(enginesTab ?? ""), (enginesTab ?? "").slice(0, 60));

const shot = await send("Page.captureScreenshot", { format: "png" });
writeFileSync(out, Buffer.from(shot.data, "base64"));

const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} passed -> ${out}`);
edge.kill();
process.exit(failed.length === 0 ? 0 : 1);
