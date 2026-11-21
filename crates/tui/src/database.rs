//! Pure helpers for the database and backups tabs: masking, schedule input and download paths.

use chrono::{DateTime, Datelike, FixedOffset, Local, Offset, Utc};
use common::{Backup, BackupSchedule, BackupStatus, DatabaseKind, Id};
use std::path::{Path, PathBuf};

pub const MASK: &str = "••••";

/// The variable the daemon stores a database's connection URL in.
pub fn url_var(kind: DatabaseKind) -> &'static str {
    match kind {
        DatabaseKind::Redis => "REDIS_URL",
        _ => "DATABASE_URL",
    }
}

/// `url` with every occurrence of the password masked, unless revealed.
pub fn masked(url: &str, password: &str, reveal: bool) -> String {
    if reveal || password.is_empty() {
        return url.to_string();
    }
    url.replace(password, MASK)
}

/// The host port offered when exposing a database: its own port shifted by 10000, so it does
/// not collide with a database installed on the server itself.
pub fn suggested_public_port(port: u16) -> u16 {
    port.checked_add(10_000).unwrap_or(port)
}

/// `04:00 local` for a UTC hour in the given local offset.
pub fn local_hour(hour_utc: u8, offset: FixedOffset) -> String {
    let minutes = (i32::from(hour_utc) * 60 + offset.local_minus_utc() / 60).rem_euclid(24 * 60);
    format!("{:02}:{:02} local", minutes / 60, minutes % 60)
}

/// The local-time hint next to the hour field of the schedule form.
pub fn hour_hint(text: &str) -> String {
    match text.trim().parse::<u8>() {
        Ok(h) if h < 24 => format!("= {}", local_hour(h, Local::now().offset().fix())),
        _ => "0-23".into(),
    }
}

/// Validate the schedule form; `storage_id` is the store picked, `None` for local.
pub fn parse_schedule(
    enabled: bool,
    hour: &str,
    keep: &str,
    storage_id: Option<Id>,
) -> Result<BackupSchedule, String> {
    let hour_utc = hour
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|h| *h < 24)
        .ok_or("hour must be 0-23 (UTC)")?;
    let keep = keep
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|k| *k > 0)
        .ok_or("keep at least 1 backup")?;
    Ok(BackupSchedule {
        enabled,
        hour_utc,
        keep,
        storage_id,
    })
}

/// Where a download goes: `~/` expands to `home`, and an existing directory or a path
/// ending in `/` gets the backup's file name appended.
pub fn download_target(input: &str, file_name: &str, home: Option<&Path>) -> PathBuf {
    let input = input.trim();
    let path = match (input.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ if input.is_empty() => PathBuf::from(file_name),
        _ => PathBuf::from(input),
    };
    if input.ends_with('/') || path.is_dir() {
        path.join(file_name)
    } else {
        path
    }
}

/// `today 14:31`, `Nov 20 03:00`, or `Nov 20 2025` for older years, in local time.
pub fn created(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let (at, now) = (at.with_timezone(&Local), now.with_timezone(&Local));
    if at.date_naive() == now.date_naive() {
        at.format("today %H:%M").to_string()
    } else if at.year() == now.year() {
        at.format("%b %d %H:%M").to_string()
    } else {
        at.format("%b %d %Y").to_string()
    }
}

/// The newest finished backup.
pub fn last_good(backups: &[Backup]) -> Option<&Backup> {
    backups
        .iter()
        .filter(|b| b.status == BackupStatus::Done)
        .max_by_key(|b| b.created_at)
}

/// `daily 03:00 UTC · 7 kept`, or `no schedule`.
pub fn schedule_summary(schedule: &BackupSchedule) -> String {
    if schedule.enabled {
        format!(
            "daily {:02}:00 UTC · {} kept",
            schedule.hour_utc, schedule.keep
        )
    } else {
        "no schedule".into()
    }
}

/// Time until the next scheduled backup: `in 12h` or `in 40m`.
pub fn next_run(schedule: &BackupSchedule, now: DateTime<Utc>) -> Option<String> {
    if !schedule.enabled {
        return None;
    }
    let today = now
        .date_naive()
        .and_hms_opt(u32::from(schedule.hour_utc), 0, 0)?
        .and_utc();
    let next = if today > now {
        today
    } else {
        today + chrono::Duration::days(1)
    };
    let mins = (next - now).num_minutes().max(1);
    Some(if mins >= 60 {
        format!("in {}h", mins / 60)
    } else {
        format!("in {mins}m")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn urls_mask_the_password_until_revealed() {
        let url = "postgres://app:s3cret@db:5432/app";
        assert_eq!(
            masked(url, "s3cret", false),
            "postgres://app:••••@db:5432/app"
        );
        assert_eq!(masked(url, "s3cret", true), url);
        assert_eq!(
            masked("redis://:pw@cache:6379", "pw", false),
            "redis://:••••@cache:6379"
        );
        assert_eq!(masked(url, "", false), url);
    }

    #[test]
    fn public_ports_shift_by_ten_thousand() {
        assert_eq!(suggested_public_port(5432), 15432);
        assert_eq!(suggested_public_port(6379), 16379);
        assert_eq!(suggested_public_port(60000), 60000);
    }

    #[test]
    fn utc_hours_convert_to_local_time() {
        let cph = FixedOffset::east_opt(3600).unwrap();
        assert_eq!(local_hour(3, cph), "04:00 local");
        assert_eq!(local_hour(23, cph), "00:00 local");
        let ny = FixedOffset::west_opt(5 * 3600).unwrap();
        assert_eq!(local_hour(2, ny), "21:00 local");
        let india = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
        assert_eq!(local_hour(0, india), "05:30 local");
    }

    #[test]
    fn schedule_input_is_validated() {
        assert_eq!(
            parse_schedule(true, " 3 ", "7", None),
            Ok(BackupSchedule {
                enabled: true,
                hour_utc: 3,
                keep: 7,
                storage_id: None,
            })
        );
        assert!(parse_schedule(true, "24", "7", None).is_err());
        assert!(parse_schedule(true, "x", "7", None).is_err());
        assert!(parse_schedule(false, "3", "0", None).is_err());
        assert!(parse_schedule(false, "3", "-1", None).is_err());
    }

    #[test]
    fn download_paths_expand_home_and_directories() {
        let home = Path::new("/home/me");
        let file = "20261120-030000-1.dump.gz";
        assert_eq!(
            download_target("~/dumps/a.gz", file, Some(home)),
            PathBuf::from("/home/me/dumps/a.gz")
        );
        assert_eq!(
            download_target("backups/", file, Some(home)),
            PathBuf::from("backups").join(file)
        );
        assert_eq!(download_target("", file, None), PathBuf::from(file));
        assert_eq!(
            download_target("./x.gz", file, None),
            PathBuf::from("./x.gz")
        );
        let dir = std::env::temp_dir();
        assert_eq!(
            download_target(dir.to_str().unwrap(), file, None),
            dir.join(file)
        );
    }

    #[test]
    fn next_run_counts_down_to_the_hour() {
        let s = BackupSchedule {
            enabled: true,
            hour_utc: 3,
            keep: 7,
            storage_id: None,
        };
        let now = Utc.with_ymd_and_hms(2026, 11, 20, 15, 0, 0).unwrap();
        assert_eq!(next_run(&s, now).as_deref(), Some("in 12h"));
        let now = Utc.with_ymd_and_hms(2026, 11, 20, 2, 20, 0).unwrap();
        assert_eq!(next_run(&s, now).as_deref(), Some("in 40m"));
        let off = BackupSchedule {
            enabled: false,
            ..s.clone()
        };
        assert_eq!(next_run(&off, now), None);
        assert_eq!(schedule_summary(&off), "no schedule");
        assert_eq!(schedule_summary(&s), "daily 03:00 UTC · 7 kept");
    }

    #[test]
    fn creation_dates_read_naturally() {
        let now = Utc::now();
        assert!(created(now, now).starts_with("today "));
        let old = Utc.with_ymd_and_hms(2020, 3, 4, 12, 0, 0).unwrap();
        assert!(created(old, now).ends_with("2020"));
    }
}
