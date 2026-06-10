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
        "ms" => 1,
        "" | "s" => 1_000,
        "m" => 60_000,
        "h" => 60 * 60_000,
        "d" => 24 * 60 * 60_000,
        _ => return Err(FlintError::InvalidDuration(value.to_string())),
    };
    number
        .checked_mul(multiplier)
        .filter(|value| *value > 0)
        .ok_or_else(|| FlintError::InvalidDuration(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duration_suffixes() {
        assert_eq!(parse_duration("100ms").unwrap(), 100);
        assert_eq!(parse_duration("10s").unwrap(), 10_000);
        assert_eq!(parse_duration("1m").unwrap(), 60_000);
        assert_eq!(parse_duration("2h").unwrap(), 7_200_000);
        assert_eq!(parse_duration("1d").unwrap(), 86_400_000);
        assert!(parse_duration("0ms").is_err());
    }
}
