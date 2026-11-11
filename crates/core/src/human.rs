//! Human-readable numbers.  The UI and the CLI share one implementation so a
//! size never reads differently in two places.

pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 1;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if value >= 100.0 { 0 } else if value >= 10.0 { 1 } else { 2 };
    format!("{:.digits$} {}", value, UNITS[unit])
}

pub fn time(epoch_seconds: i64) -> String {
    let days = days_from_civil_of(epoch_seconds);
    let secs = epoch_seconds.rem_euclid(86_400);
    let (y, m, d) = days;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

pub fn duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "—".into();
    }
    let s = seconds.round() as i64;
    if s < 60 {
        return format!("{s} 秒");
    }
    if s < 3600 {
        return format!("{} 分 {} 秒", s / 60, s % 60);
    }
    format!("{} 時 {} 分", s / 3600, (s % 3600) / 60)
}

fn days_from_civil_of(epoch: i64) -> (i64, i64, i64) {
    let z = epoch.div_euclid(86_400) + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// FNV-1a, so a scratch folder name is stable across runs without pulling in a
/// hashing crate for one call site.
pub fn simple_hash(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}
