use anyhow::{anyhow, Context, Result};

pub fn remove_0x(s: &str) -> String {
    s.trim_matches('\"')
        .strip_prefix("0x")
        .unwrap_or(s)
        .to_string()
}

pub fn add_0x(s: String) -> String {
    "0x".to_string() + &s
}

pub fn parse_value(s: &str) -> Result<Vec<u8>> {
    let s = remove_0x(s);
    if s.len() > 64 {
        return Err(anyhow!("can't parse value, the given str is too long"));
    }
    // padding 0 to 32 bytes
    let padded = format!("{s:0>64}");
    hex::decode(padded).map_err(|e| anyhow!("invalid value: {e}"))
}

pub fn parse_data(s: &str) -> Result<Vec<u8>> {
    hex::decode(remove_0x(s)).context("invalid hex input")
}

pub fn display_value(s: &str) -> Option<String> {
    if s.len() != 64 {
        return None;
    }

    let int_part = &s[..46].trim_start_matches('0');
    let int_part = if int_part.is_empty() { "0" } else { int_part };

    let frac_part = &s[46..];

    if frac_part.chars().all(|ch| ch == '0') {
        Some(int_part.to_string())
    } else {
        Some(format!("{}.{}", int_part, frac_part))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_value() {
        let res = display_value("0000000001234567890123456789012345678901234567000000000000000000");
        assert_eq!(res.unwrap(), "1234567890123456789012345678901234567");

        let res = display_value("0000000001234567890123456789012345678901234567000000000000000001");
        assert_eq!(
            res.unwrap(),
            "1234567890123456789012345678901234567.000000000000000001"
        );

        let res = display_value("0000000000000000000000000000000000000000000001000000000000000000");
        assert_eq!(res.unwrap(), "1");
    }
}
