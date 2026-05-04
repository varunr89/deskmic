use anyhow::{anyhow, Result};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

/// Parsed identity of a recorder file like `260429_0909.mp3`
/// or `251121_1710_01.mp3` (a continuation chunk produced by the device).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRecorderName {
    pub start: NaiveDateTime,
    /// `None` for the first/only file, `Some(n)` for `_NN` continuations.
    pub split_index: Option<u32>,
}

/// Parse `YYMMDD_HHMM(_NN)?.mp3`. Two-digit year is interpreted as 2000-2099.
pub fn parse(filename: &str) -> Result<ParsedRecorderName> {
    let stem = filename
        .strip_suffix(".mp3")
        .or_else(|| filename.strip_suffix(".MP3"))
        .ok_or_else(|| anyhow!("not an .mp3 file: {}", filename))?;

    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Err(anyhow!("unexpected filename shape: {}", filename));
    }
    let date_part = parts[0];
    let time_part = parts[1];
    let split_index = match parts.get(2) {
        Some(s) => Some(s.parse::<u32>().map_err(|_| {
            anyhow!("split index not numeric: {}", filename)
        })?),
        None => None,
    };

    if date_part.len() != 6 || time_part.len() != 4 {
        return Err(anyhow!("date/time fields wrong length: {}", filename));
    }

    let year: i32 = 2000 + date_part[0..2].parse::<i32>()?;
    let month: u32 = date_part[2..4].parse()?;
    let day: u32 = date_part[4..6].parse()?;
    let hour: u32 = time_part[0..2].parse()?;
    let minute: u32 = time_part[2..4].parse()?;

    let date = NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| anyhow!("invalid date in {}", filename))?;
    let time = NaiveTime::from_hms_opt(hour, minute, 0)
        .ok_or_else(|| anyhow!("invalid time in {}", filename))?;
    Ok(ParsedRecorderName {
        start: NaiveDateTime::new(date, time),
        split_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_name() {
        let p = parse("260429_0909.mp3").unwrap();
        assert_eq!(p.start.date(), NaiveDate::from_ymd_opt(2026, 4, 29).unwrap());
        assert_eq!(p.start.time(), NaiveTime::from_hms_opt(9, 9, 0).unwrap());
        assert_eq!(p.split_index, None);
    }

    #[test]
    fn parses_split_continuation() {
        let p = parse("251121_1710_01.mp3").unwrap();
        assert_eq!(p.split_index, Some(1));
    }

    #[test]
    fn rejects_non_mp3() {
        assert!(parse("260429_0909.wav").is_err());
    }

    #[test]
    fn rejects_bad_shape() {
        assert!(parse("hello.mp3").is_err());
        assert!(parse("260429.mp3").is_err());
        assert!(parse("260429_0909_01_02.mp3").is_err());
    }

    #[test]
    fn rejects_invalid_date() {
        assert!(parse("261332_0909.mp3").is_err());
    }
}
