use color_eyre::eyre::{eyre, Context, Result};

pub fn remove_quotes_and_0x(s: &str) -> String {
    let new_s = s.trim_matches('\"');
    new_s.strip_prefix("0x").unwrap_or(new_s).to_string()
}

pub fn add_0x(s: String) -> String {
    "0x".to_string() + &s
}

pub fn parse_value(s: &str) -> Result<Vec<u8>> {
    let s = remove_quotes_and_0x(s);
    if s.len() > 64 {
        return Err(eyre!("can't parse value, the given str is too long"));
    }
    // padding 0 to 32 bytes
    let padded = format!("{s:0>64}");
    hex::decode(padded).map_err(|e| eyre!("invalid value: {e}"))
}

pub fn parse_data(s: &str) -> Result<Vec<u8>> {
    hex::decode(remove_quotes_and_0x(s)).context("invalid hex input")
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

pub mod bytes_hex {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{}", hex::encode(bytes)))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        hex::decode(s.trim_start_matches("0x")).map_err(serde::de::Error::custom)
    }
}

pub mod value_hex {
    use ethabi::ethereum_types::U256;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &(Vec<u8>, U256), serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!(
            "0x{},{}",
            hex::encode(value.0.clone()),
            &value.1.to_string()
        ))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<(Vec<u8>, U256), D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let str = s.trim_start_matches("0x").split(',').collect::<Vec<&str>>();
        let value_str = if let Some(value_str) = str.first() {
            value_str
        } else {
            return Err(serde::de::Error::custom("invalid value"));
        };
        let u256_str = if let Some(u256_str) = str.get(1) {
            u256_str
        } else {
            return Err(serde::de::Error::custom("invalid value"));
        };
        let value = hex::decode(value_str).map_err(serde::de::Error::custom)?;
        let u256 = U256::from_dec_str(u256_str).map_err(serde::de::Error::custom)?;
        Ok((value, u256))
    }
}

#[test]
fn test_u256() {
    let q = "0x0000000000000000000000000000000000000000000000000000000000387165";
    let q = q.trim_start_matches("0x");
    let q = hex::decode(q).unwrap();
    let q = cita_tool::U256::from_big_endian(&q);
    println!("{:?}", q);
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
