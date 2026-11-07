//! Canonical local-date/time helper.
//!
//! Historically this logic existed three times (`repository::chat_history` on Windows and
//! Unix, and `llm::usage` on all platforms) with subtly different output. Consolidating
//! them here guarantees a single, tested timestamp format everywhere:
//!
//! ```text
//! ("YYYY-MM-DD", "HH:MM:SS±HH:MM")
//! ```
//!
//! The `usage` copy used to omit the timezone offset entirely, which made stored
//! timestamps ambiguous when the database was read from another timezone.

/// Returns the current local date and time as `(YYYY-MM-DD, HH:MM:SS±HH:MM)`.
///
/// The second element always carries the local UTC offset so timestamps remain
/// unambiguous regardless of where the database is later read.
pub fn get_local_date_and_time() -> (String, String) {
    match local_now() {
        Some((date, time)) => (date, time),
        // Falling back to a fixed epoch keeps callers total instead of panicking on
        // platforms or environments where the OS clock cannot be read.
        None => ("1970-01-01".to_string(), "00:00:00+00:00".to_string()),
    }
}

/// Formats a UTC offset expressed in seconds as `±HH:MM`.
fn format_offset(total_seconds: i64) -> String {
    let sign = if total_seconds >= 0 { '+' } else { '-' };
    let abs = total_seconds.abs();
    let hours = abs / 3600;
    let minutes = (abs % 3600) / 60;
    format!("{sign}{hours:02}:{minutes:02}")
}

#[cfg(unix)]
fn local_now() -> Option<(String, String)> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // SAFETY: `tm` is a valid, zero-initialized `libc::tm`; `localtime_r` writes into it
    // and returns the same pointer on success or null on failure (checked below).
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return None;
        }
        let date = format!(
            "{:04}-{:02}-{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday
        );
        let time = format!(
            "{:02}:{:02}:{:02}{}",
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
            format_offset(tm.tm_gmtoff)
        );
        Some((date, time))
    }
}

#[cfg(windows)]
fn local_now() -> Option<(String, String)> {
    use windows::Win32::System::SystemInformation::GetLocalTime;

    #[repr(C)]
    struct SystemTime {
        w_year: u16,
        w_month: u16,
        w_day_of_week: u16,
        w_day: u16,
        w_hour: u16,
        w_minute: u16,
        w_second: u16,
        w_milliseconds: u16,
    }

    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTime,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTime,
        daylight_bias: i32,
    }

    // `GetTimeZoneInformation` lives in `Win32_System_Time`, which is not enabled in
    // Cargo.toml, so it is declared directly to avoid pulling in a new crate feature.
    unsafe extern "system" {
        fn GetTimeZoneInformation(lp_time_zone_information: *mut TimeZoneInformation) -> u32;
    }

    // The `windows` crate avoids needing our hand-rolled `SystemTime` layout.
    let st = unsafe { GetLocalTime() };
    let date = format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay);

    // SAFETY: `tzi` is zero-initialized and the API writes into the provided pointer.
    // A return value of 0xFFFF_FFFF (TIME_ZONE_ID_INVALID) means the struct contents are
    // undefined, so in that case we fall back to UTC rather than trusting garbage.
    let offset = unsafe {
        let mut tzi = std::mem::MaybeUninit::<TimeZoneInformation>::zeroed();
        match GetTimeZoneInformation(tzi.as_mut_ptr()) {
            u32::MAX => None,
            _ => {
                let tzi = tzi.assume_init();
                // `bias` is defined as UTC = local + bias (minutes), so the local offset
                // is the negation, converted to seconds.
                Some(-(tzi.bias as i64) * 60)
            }
        }
    }
    .unwrap_or(0);

    let time = format!(
        "{:02}:{:02}:{:02}{}",
        st.wHour,
        st.wMinute,
        st.wSecond,
        format_offset(offset)
    );
    Some((date, time))
}

#[cfg(not(any(unix, windows)))]
fn local_now() -> Option<(String, String)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_formats_positive_zero_and_negative() {
        assert_eq!(format_offset(0), "+00:00");
        assert_eq!(format_offset(7 * 3600), "+07:00");
        assert_eq!(format_offset(5 * 3600 + 30 * 60), "+05:30");
        assert_eq!(format_offset(-(3 * 3600 + 30 * 60)), "-03:30");
        assert_eq!(format_offset(-(45 * 60)), "-00:45");
    }

    #[test]
    fn local_date_and_time_has_stable_shape() {
        let (date, time) = get_local_date_and_time();
        // YYYY-MM-DD
        assert_eq!(date.len(), 10, "unexpected date shape: {date}");
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
        // HH:MM:SS±HH:MM => 8 + 6 = 14 chars
        assert_eq!(time.len(), 14, "unexpected time shape: {time}");
        let (clock, offset) = time.split_at(8);
        assert!(!offset.is_empty(), "time must carry a UTC offset: {time}");
        assert!(
            offset.starts_with('+') || offset.starts_with('-'),
            "offset must be signed: {time}"
        );
        assert_eq!(&clock[2..3], ":");
        assert_eq!(&clock[5..6], ":");
    }
}
