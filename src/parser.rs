use std::time::{Duration, SystemTime};

use chrono::{Local, NaiveTime};
use regex::Regex;
use snafu::{ResultExt, Snafu};

use crate::task::TaskType;

#[derive(Debug, Snafu)]
pub enum ParseError {
    #[snafu(display("Invalid time format: {}", msg))]
    InvalidFormat { msg: String },
    #[snafu(display("Invalid duration: {}", msg))]
    InvalidDuration { msg: String },
}

pub fn parse_time_input(input: &str) -> Result<(String, TaskType), ParseError> {
    let re = Regex::new(r"^(.*?)(?:#(.+))?$").unwrap();
    let caps = re.captures(input).ok_or_else(|| ParseError::InvalidFormat {
        msg: "Invalid input format".to_string(),
    })?;

    let time_str = caps.get(1).unwrap().as_str().trim();
    let name = caps.get(2).map_or("未命名", |m| m.as_str()).to_string();

    if let Some(time_str) = time_str.strip_prefix('@') {
        // 处理截止时间格式 (@HH:MM)
        let time = NaiveTime::parse_from_str(time_str, "%H:%M").map_err(|_| ParseError::InvalidFormat {
            msg: "Invalid time format".to_string(),
        })?;

        let now = Local::now();
        let mut deadline = now.date_naive().and_time(time);
        if deadline < now.naive_local() {
            deadline += chrono::Duration::days(1);
        }

        Ok((
            name,
            TaskType::Deadline(deadline.and_local_timezone(Local).unwrap().into()),
        ))
    } else {
        // 处理时间段格式 (1h30m)
        let mut duration = Duration::ZERO;
        let re = Regex::new(r"(\d+)([hm])").unwrap();

        for cap in re.captures_iter(time_str) {
            let value: u64 = cap[1].parse().map_err(|_| ParseError::InvalidDuration {
                msg: "Invalid number".to_string(),
            })?;
            let unit = &cap[2];

            match unit {
                "h" => duration += Duration::from_secs(value * 3600),
                "m" => duration += Duration::from_secs(value * 60),
                _ => {
                    return Err(ParseError::InvalidDuration {
                        msg: "Invalid time unit".to_string(),
                    });
                }
            }
        }

        if duration == Duration::ZERO {
            return Err(ParseError::InvalidDuration {
                msg: "Duration cannot be zero".to_string(),
            });
        }

        Ok((name, TaskType::Duration(duration)))
    }
}
