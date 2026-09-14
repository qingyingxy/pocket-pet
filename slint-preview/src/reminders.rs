use chrono::{DateTime, Days, Local, TimeZone, Utc};
use std::time::Duration;
pub const HALF_HOUR: u64 = 30 * 60;
pub fn now() -> u64 {
    Utc::now().timestamp().max(0) as u64
}
pub fn is_due(at: Option<u64>, done: bool, now: u64) -> bool {
    !done && at.is_some_and(|at| at <= now)
}
// Reconcile clock changes/resume every 30s only while future reminders exist.
pub fn next_delay(times: impl Iterator<Item = u64>, now: u64) -> Option<Duration> {
    times
        .filter(|at| *at > now)
        .min()
        .map(|at| Duration::from_secs((at - now).clamp(1, 30)))
}
pub fn later_today(now: DateTime<Local>) -> Option<u64> {
    let today = now.date_naive();
    let evening = Local
        .from_local_datetime(&today.and_hms_opt(21, 0, 0)?)
        .earliest()?;
    let target = if evening > now {
        evening
    } else {
        Local
            .from_local_datetime(&today.checked_add_days(Days::new(1))?.and_hms_opt(9, 0, 0)?)
            .earliest()?
    };
    u64::try_from(target.timestamp()).ok()
}
pub fn label(at: Option<u64>, now: u64) -> String {
    let Some(at) = at else {
        return "不提醒".into();
    };
    if at <= now {
        return "到时间了".into();
    }
    let Some(time) = i64::try_from(at)
        .ok()
        .and_then(|t| DateTime::from_timestamp(t, 0))
    else {
        return "提醒时间无效".into();
    };
    format!("{} 提醒", time.with_timezone(&Local).format("%m/%d %H:%M"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn due_and_scheduler_boundaries() {
        assert!(!is_due(None, false, 100));
        assert!(!is_due(Some(100), true, 100));
        assert!(is_due(Some(100), false, 100));
        assert!(is_due(Some(90), false, 100));
        assert!(!is_due(Some(101), false, 100));
        assert_eq!(next_delay([90, 100].into_iter(), 100), None);
        assert_eq!(
            next_delay([101, 200].into_iter(), 100),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            next_delay([200].into_iter(), 100),
            Some(Duration::from_secs(30))
        );
    }
    #[test]
    fn evening_rolls_to_next_morning_after_21() {
        let early = Local.with_ymd_and_hms(2026, 9, 14, 20, 0, 0).unwrap();
        let late = Local.with_ymd_and_hms(2026, 9, 14, 21, 0, 0).unwrap();
        assert_eq!(
            later_today(early),
            Some(
                Local
                    .with_ymd_and_hms(2026, 9, 14, 21, 0, 0)
                    .unwrap()
                    .timestamp() as u64
            )
        );
        assert_eq!(
            later_today(late),
            Some(
                Local
                    .with_ymd_and_hms(2026, 9, 15, 9, 0, 0)
                    .unwrap()
                    .timestamp() as u64
            )
        );
    }
}
