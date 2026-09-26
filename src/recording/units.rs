//! Durations and sizes as people write them: `8h`, `1h30m`, `512MiB`. Shared by `rozi record`'s
//! flags and the `[recording]` config, so both read one spelling.

/// A duration such as `90s`, `30m`, `8h`, `1h30m`, or `2d`, in milliseconds. A bare number is
/// seconds.
pub fn parse_duration_ms(value: &str) -> Result<u64, String> {
    let invalid = || format!("`{value}` is not a duration such as 30s, 8h, or 1h30m");
    if let Ok(seconds) = value.parse::<u64>() {
        return seconds.checked_mul(1_000).ok_or_else(invalid);
    }
    let mut total: u64 = 0;
    let mut rest = value;
    if rest.is_empty() {
        return Err(invalid());
    }
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(invalid)?;
        if digits == 0 {
            return Err(invalid());
        }
        let number: u64 = rest[..digits].parse().map_err(|_| invalid())?;
        rest = &rest[digits..];
        let (unit, multiplier) = [
            ("ms", 1),
            ("s", 1_000),
            ("m", 60_000),
            ("h", 3_600_000),
            ("d", 86_400_000),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit) && !(*unit == "m" && rest.starts_with("ms")))
        .ok_or_else(invalid)?;
        rest = &rest[unit.len()..];
        total = number
            .checked_mul(multiplier)
            .and_then(|ms| total.checked_add(ms))
            .ok_or_else(invalid)?;
    }
    Ok(total)
}

/// A size such as `512MiB`, `1GiB`, `100MB`, or a byte count. `K`, `M`, and `G` alone are binary.
pub fn parse_size(value: &str) -> Result<u64, String> {
    let invalid = || format!("`{value}` is not a size such as 512MiB or 1GiB");
    let digits = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let number: u64 = value[..digits].parse().map_err(|_| invalid())?;
    let multiplier: u64 = match value[digits..].trim() {
        "" | "B" => 1,
        "K" | "KiB" => 1 << 10,
        "M" | "MiB" => 1 << 20,
        "G" | "GiB" => 1 << 30,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        _ => return Err(invalid()),
    };
    number.checked_mul(multiplier).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_sizes_read_the_way_people_write_them() {
        assert_eq!(parse_duration_ms("8h"), Ok(8 * 3_600_000));
        assert_eq!(parse_duration_ms("1h30m"), Ok(90 * 60_000));
        assert_eq!(parse_duration_ms("500ms"), Ok(500));
        assert_eq!(parse_duration_ms("2d"), Ok(2 * 86_400_000));
        assert_eq!(parse_duration_ms("90"), Ok(90_000));
        for bad in ["", "h", "8x", "1.5h", "-1s"] {
            assert!(parse_duration_ms(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_size("512MiB"), Ok(512 << 20));
        assert_eq!(parse_size("1GiB"), Ok(1 << 30));
        assert_eq!(parse_size("100MB"), Ok(100_000_000));
        assert_eq!(parse_size("4096"), Ok(4096));
        assert!(parse_size("1TB").is_err() && parse_size("MiB").is_err());
    }
}
