use crate::FlintError;

pub fn parse_duration(value: &str) -> Result<u64, FlintError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(FlintError::InvalidDuration(value.to_string()));
    }
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split);
    let number = number
        .parse::<u64>()
        .map_err(|_| FlintError::InvalidDuration(value.to_string()))?;
    let multiplier = match suffix {
        "" | "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 60 * 60 * 24,
        _ => return Err(FlintError::InvalidDuration(value.to_string())),
    };
    Ok(number * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duration_suffixes() {
        assert_eq!(parse_duration("10s").unwrap(), 10);
        assert_eq!(parse_duration("1m").unwrap(), 60);
        assert_eq!(parse_duration("2h").unwrap(), 7200);
        assert_eq!(parse_duration("1d").unwrap(), 86400);
        assert!(parse_duration("100ms").is_err());
    }
}
