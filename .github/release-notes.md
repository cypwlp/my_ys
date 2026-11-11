## QuickArc v0.1.1

首個跨平台版本。Windows、macOS 與 Linux 都由 GitHub Actions 從同一份原始碼建置。

### 平台檔案

- **Windows x64**：可攜版 `.exe` 與 NSIS 安裝程式
- **macOS Universal**：單一 `.dmg`，同時支援 Intel 與 Apple Silicon
- **Linux x86_64**：`.AppImage` 與 Debian／Ubuntu `.deb`

### 本版重點

- 補齊 macOS／Linux 建置與發版流程。
- 新增 macOS `icns`、Linux PNG 與跨平台圖示資產。
- macOS／Linux 不再顯示只適用於 Windows 的登錄檔整合頁。
- Windows 版本仍保留原本的可攜式執行檔與免安裝使用方式。

### 注意事項

- 本次產物尚未做程式碼簽章或 Apple notarization。Windows SmartScreen 與 macOS Gatekeeper 可能顯示警告。
- macOS 首次啟動若被攔截，請在 Finder 對 App 按右鍵選擇「打開」。
- Linux `.deb` 適合 Debian／Ubuntu；`.AppImage` 建議在具備 GTK／WebKitGTK 的桌面環境使用。
- 所有下載檔的 SHA-256 值列於 `SHA256SUMS`。