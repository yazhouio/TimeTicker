use std::time::{Duration, SystemTime};

use chrono::{Local, NaiveTime};
use regex::Regex;
use snafu::{ResultExt, OptionExt, Backtrace}; // Ensure Backtrace is imported if used directly, though snafu macros handle it.
use crate::error::{Result, Error, RegexCompileSnafu, InvalidInputFormatSnafu, MissingTimeInputSnafu, ChronoParseSnafu, TimezoneConversionSnafu, ParseNumberSnafu, InvalidDurationUnitSnafu, ZeroDurationSnafu};
use crate::task::TaskType;


pub fn parse_time_input(input: &str) -> Result<(String, TaskType)> {
    let re = Regex::new(r"^(.*?)(?:#(.+))?$").context(RegexCompileSnafu)?;
    let caps = re.captures(input).context(InvalidInputFormatSnafu { msg: "Input does not match expected format (time_string#name)".to_string() })?;

    let time_str = caps.get(1)
        .map(|m| m.as_str().trim())
        .filter(|s| !s.is_empty()) // Ensure time_str is not empty after trim
        .context(MissingTimeInputSnafu { msg: "Time string is missing or empty".to_string() })?;

    let name = caps.get(2).map_or("未命名", |m| m.as_str().trim()).to_string();

    if let Some(deadline_time_str) = time_str.strip_prefix('@') {
        // 处理截止时间格式 (@HH:MM)
        let time = NaiveTime::parse_from_str(deadline_time_str, "%H:%M").context(ChronoParseSnafu)?;

        let now = Local::now();
        let mut deadline_datetime_naive = now.date_naive().and_time(time);
        if deadline_datetime_naive < now.naive_local() {
            deadline_datetime_naive += chrono::Duration::days(1);
        }
        
        let deadline_datetime_local = deadline_datetime_naive.and_local_timezone(Local).single()
            .context(TimezoneConversionSnafu { msg: format!("Failed to convert NaiveDateTime {} to local timezone", deadline_datetime_naive) })?;

        Ok((name, TaskType::Deadline(deadline_datetime_local.into())))
    } else {
        // 处理时间段格式 (1h30m)
        let mut total_duration = Duration::ZERO;
        let re_duration = Regex::new(r"(\d+)\s*([hm])").context(RegexCompileSnafu)?; // Allow optional space

        if !re_duration.is_match(time_str) && !time_str.is_empty() {
             // If it's not a deadline and not a valid duration pattern, but not empty, it's an invalid format.
            return InvalidInputFormatSnafu { msg: format!("Invalid duration format: '{}'", time_str) }.fail();
        }


        for cap in re_duration.captures_iter(time_str) {
            let value_str = cap.get(1).map_or("", |m| m.as_str());
            let value: u64 = value_str.parse().context(ParseNumberSnafu)?;
            
            let unit = cap.get(2).map_or("", |m| m.as_str());

            match unit {
                "h" => total_duration += Duration::from_secs(value * 3600),
                "m" => total_duration += Duration::from_secs(value * 60),
                _ => return InvalidDurationUnitSnafu { unit: unit.to_string() }.fail(),
            }
        }

        if total_duration == Duration::ZERO && !time_str.is_empty() { // Only error if input was provided but parsed to zero
             // Check if time_str was actually empty or just didn't match.
             // If time_str was not empty but duration is zero, it means it might have contained invalid parts.
             // However, if re_duration found no matches at all, and time_str wasn't a deadline, it's an invalid format.
             // The re_duration.is_match check above should handle cases where no duration parts are found.
             // This ZeroDurationSnafu is for cases like "0h0m".
            return ZeroDurationSnafu.fail();
        }
         if total_duration == Duration::ZERO && time_str.is_empty() {
             // If time_str itself was empty (after stripping #name), it's a missing time input.
             return MissingTimeInputSnafu { msg: "Time string was empty after removing name part".to_string() }.fail();
         }


        Ok((name, TaskType::Duration(total_duration)))
    }
}
