const REPO = "cypwlp/my_ys";
const RELEASE_API = `https://api.github.com/repos/${REPO}/releases/latest`;
const RELEASE_PAGE = `https://github.com/${REPO}/releases/latest`;
const ALL_RELEASES = `https://github.com/${REPO}/releases`;
const MIRROR_HOSTS = new Set(["kumoarc.cypilu.cloud"]);
const MIRROR_MANIFEST = "./downloads/manifest.json";
const DOWNLOAD_KEYS = ["windows-portable", "windows-setup", "macos", "linux-appimage", "linux-deb", "checksums"];

const has = (name, token) => name.toLowerCase().includes(token);

function setVersion(value) {
  const version = value || "latest";
  document.querySelectorAll("[data-release-version]").forEach((element) => {
    element.textContent = version.startsWith("v") ? version : `v${version}`;
  });
}

function setDownload(key, url) {
  if (!url) return;
  document.querySelectorAll(`[data-download="${key}"]`).forEach((link) => {
    link.href = url;
    link.removeAttribute("aria-disabled");
  });
}

function assetFor(assets, key) {
  const names = assets.map((asset) => ({ ...asset, lower: asset.name.toLowerCase() }));
  const find = (predicate) => names.find(predicate)?.browser_download_url;

  switch (key) {
    case "windows-portable":
      return find((asset) => asset.lower.endsWith("windows-x64.exe") && !has(asset.lower, "setup"));
    case "windows-setup":
      return find((asset) => asset.lower.endsWith("windows-x64-setup.exe"));
    case "macos":
      return find((asset) => asset.lower.endsWith(".dmg"));
    case "linux-appimage":
      return find((asset) => asset.lower.endsWith(".appimage"));
    case "linux-deb":
      return find((asset) => asset.lower.endsWith(".deb"));
    case "checksums":
      return find((asset) => asset.lower === "sha256sums");
    default:
      return undefined;
  }
}

async function loadMirrorManifest() {
  if (!MIRROR_HOSTS.has(window.location.hostname)) return false;

  try {
    const response = await fetch(MIRROR_MANIFEST, { cache: "no-store" });
    if (!response.ok) return false;
    const manifest = await response.json();
    setVersion(manifest.version);

    for (const [key, filename] of Object.entries(manifest.assets || {})) {
      if (!DOWNLOAD_KEYS.includes(key) || !filename) continue;
      setDownload(key, new URL(`./downloads/${encodeURIComponent(filename)}`, window.location.href).href);
    }

    setDownload("release-page", ALL_RELEASES);
    return true;
  } catch (error) {
    console.warn("QuickArc: mirror manifest unavailable, falling back to GitHub.", error);
    return false;
  }
}

async function loadLatestRelease() {
  for (const element of document.querySelectorAll("[data-year]")) {
    element.textContent = String(new Date().getFullYear());
  }

  if (await loadMirrorManifest()) return;

  try {
    const response = await fetch(RELEASE_API, {
      headers: { Accept: "application/vnd.github+json" },
      cache: "no-store",
    });
    if (!response.ok) throw new Error(`GitHub API ${response.status}`);

    const release = await response.json();
    setVersion(release.tag_name);

    setDownload("release-page", ALL_RELEASES);
    for (const key of DOWNLOAD_KEYS) {
      setDownload(key, assetFor(release.assets || [], key));
    }
  } catch (error) {
    console.warn("QuickArc: falling back to the latest release page.", error);
    document.querySelectorAll("[data-download]").forEach((link) => {
      link.href = RELEASE_PAGE;
    });
  }
}

loadLatestRelease();