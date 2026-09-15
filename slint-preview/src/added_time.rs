use chrono::{DateTime, Local};

pub fn labels(created: Option<u64>, now: u64) -> (String, String) {
    let date = created
        .and_then(|v| i64::try_from(v).ok())
        .and_then(|v| DateTime::from_timestamp(v, 0))
        .map(|v| v.with_timezone(&Local));
    let Some(date) = date else {
        return (String::new(), "加入时间未记录".into());
    };
    let current = DateTime::from_timestamp(now.min(i64::MAX as u64) as i64, 0)
        .unwrap_or_else(|| date.to_utc())
        .with_timezone(&Local);
    let age = now.saturating_sub(created.unwrap());
    let days = current
        .date_naive()
        .signed_duration_since(date.date_naive())
        .num_days();
    let relative = if days == 1 {
        "昨天加入".into()
    } else if days != 0 {
        date.format("%Y/%m/%d 加入").to_string()
    } else if age < 60 {
        "刚刚加入".into()
    } else if age < 3600 {
        format!("{} 分钟前加入", age / 60)
    } else {
        format!("{} 小时前加入", age / 3600)
    };
    (
        relative,
        date.format("加入于 %Y 年 %-m 月 %-d 日 %H:%M").to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    #[test]
    fn relative_time_and_legacy() {
        let now = Local
            .with_ymd_and_hms(2026, 9, 15, 14, 30, 0)
            .unwrap()
            .timestamp() as u64;
        assert_eq!(labels(None, now).1, "加入时间未记录");
        assert_eq!(labels(Some(now - 30), now).0, "刚刚加入");
        assert_eq!(labels(Some(now - 1200), now).0, "20 分钟前加入");
        assert_eq!(labels(Some(now - 7200), now).0, "2 小时前加入");
        assert_eq!(labels(Some(now - 86400), now).0, "昨天加入");
        assert_eq!(labels(Some(u64::MAX), now).1, "加入时间未记录");
    }
}
