import type { Entry } from "./types";

/** Mirrors qarc_core::browse::children.  Grouping happens in the UI so a
 *  double-click costs no IPC round trip; the Rust twin is what the tests lock
 *  down, and these two must stay in step. */
export function childrenOf(entries: Entry[], prefix: string): Entry[] {
  const folders = new Map<string, { size: number; packed: number }>();
  const files: Entry[] = [];
  for (const e of entries) {
    const path = e.path.replace(/\\/g, "/");
    if (!path.startsWith(prefix)) continue;
    const rest = path.slice(prefix.length);
    if (!rest) continue;
    // Escaping members keep their full internal path: folding them into a folder
    // named `..` would look like ordinary navigation.
    if (rest.startsWith("../") || rest.startsWith("/")) {
      files.push({ ...e, name: path.replace(/\/$/, ""), path, unsafePath: true });
      continue;
    }
    if (e.isDir && !rest.includes("/")) {
      // A directory row whose stored path has no trailing separator, which is how
      // 7z reports folders.
      const key = `${prefix}${rest}/`;
      if (!folders.has(key)) folders.set(key, { size: 0, packed: 0 });
      continue;
    }
    const slash = rest.indexOf("/");
    if (slash >= 0) {
      const key = `${prefix}${rest.slice(0, slash)}/`;
      const row = folders.get(key) ?? { size: 0, packed: 0 };
      if (rest.length > slash + 1) {
        row.size += e.size;
        row.packed += e.packed ?? 0;
      }
      folders.set(key, row);
    } else if (rest.endsWith("/")) {
      const key = `${prefix}${rest}`;
      if (!folders.has(key)) folders.set(key, { size: 0, packed: 0 });
    } else {
      files.push({ ...e, name: rest, path: `${prefix}${rest}` });
    }
  }
  const out: Entry[] = [...folders.entries()].map(([path, agg]) => ({
    name: path.slice(prefix.length).replace(/\/$/, ""),
    path,
    isDir: true,
    size: agg.size,
    packed: agg.packed > 0 ? agg.packed : null,
    mtime: null,
    attrs: "D",
    encrypted: false,
    codec: null,
    unsafePath: false,
  }));
  out.push(...files);
  out.sort((a, b) => (a.isDir !== b.isDir ? (a.isDir ? -1 : 1) : a.name.localeCompare(b.name, "zh-Hant")));
  return out;
}
