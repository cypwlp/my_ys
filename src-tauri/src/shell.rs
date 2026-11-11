//! Windows shell integration and the recycle bin.
//!
//! Two rules carried over from the .NET build, both learned the hard way:
//!   * HKCU only, with a `QuickArc.` verb prefix, so removal is one registry tree
//!     and no other application's keys are ever touched.
//!   * Never build a PowerShell string to move a file to the bin.  Archive- and
//!     filename-controlled text in a shell command is an injection surface; this
//!     calls SHFileOperationW directly.

#[cfg(windows)]
mod imp {
    use windows::core::{w, BOOL, PCWSTR};
    use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
    use windows::Win32::System::Registry::{
        RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    };
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT,
        SHFILEOPSTRUCTW,
    };

    /// Double-null terminated, the shape SHFileOperationW expects for pFrom.
    fn multi(paths: &[String]) -> Vec<u16> {
        let mut buf: Vec<u16> = Vec::new();
        for p in paths {
            buf.extend(p.encode_utf16());
            buf.push(0);
        }
        buf.push(0);
        buf
    }

    /// Move to the recycle bin.  Returns Err when the shell refuses, which the
    /// caller must surface instead of silently deleting.
    pub fn recycle(paths: &[String]) -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let buffer = multi(paths);
        let mut op = SHFILEOPSTRUCTW {
            hwnd: HWND::default(),
            wFunc: FO_DELETE,
            pFrom: PCWSTR(buffer.as_ptr()),
            pTo: PCWSTR::null(),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT).0 as u16,
            fAnyOperationsAborted: BOOL::default(),
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: PCWSTR::null(),
        };
        let code = unsafe { SHFileOperationW(&mut op) };
        if code != 0 {
            return Err(format!("SHFileOperationW 回傳 {code}"));
        }
        if op.fAnyOperationsAborted.0 != 0 {
            return Err("使用者取消了回收桶操作".into());
        }
        Ok(())
    }

    /// HKCU classes tree for "open with QuickArc".  Written as a preview string
    /// first: the installer never touches the registry until the user opts in.
    pub fn reg_preview(exe: &str) -> String {
        let exe = exe.replace('\\', "\\\\");
        let mut out = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
        for (key, name, verb) in [
            ("*", "速壓. 智慧解壓", "smart"),
            ("Directory", "速壓. 壓縮為 7z", "zip7"),
        ] {
            out.push_str(&format!(
                "[HKEY_CURRENT_USER\\Software\\Classes\\{key}\\shell\\QuickArc.{verb}]\r\n\"MUIVerb\"=\"{name}\"\r\n\"Icon\"=\"{exe}\"\r\n\r\n\
                 [HKEY_CURRENT_USER\\Software\\Classes\\{key}\\shell\\QuickArc.{verb}\\command]\r\n@=\"\\\"{exe}\\\" \\\"%1\\\"\"\r\n\r\n",
                verb = verb,
                name = name,
                key = key,
                exe = exe
            ));
        }
        out
    }

    pub fn installed() -> bool {
        let mut h = HKEY::default();
        let code = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                w!("Software\\Classes\\*\\shell\\QuickArc.smart"),
                None,
                KEY_READ,
                &mut h,
            )
        };
        code == ERROR_SUCCESS
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn recycle(paths: &[String]) -> Result<(), String> {
        // Without a shell API the only safe fallback is a trash directory, and a
        // half-implemented trash is worse than an explicit refusal.
        Err(format!(
            "此平台的資源回收桶尚未實作（{} 個項目未處理）",
            paths.len()
        ))
    }

    pub fn reg_preview(_exe: &str) -> String {
        "# Linux/macOS: 桌面整合尚未實作，不會寫入任何檔案".to_string()
    }

    pub fn installed() -> bool {
        false
    }
}

pub use imp::{installed, recycle, reg_preview};
