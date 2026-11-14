use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable identifier for a project. Generated from the seed and the creation
/// time, serialised as 32 hexadecimal characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId(pub u128);

impl ProjectId {
    pub fn generate(seed: u64, now_unix: u64) -> Self {
        let hi = (u128::from(seed) << 64) | u128::from(now_unix);
        let lo = (u128::from(seed.rotate_left(17)) << 32) | u128::from((seed ^ now_unix) as u32);
        Self(hi ^ lo.rotate_left(29))
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl std::str::FromStr for ProjectId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        u128::from_str_radix(s, 16).map(Self)
    }
}

impl Serialize for ProjectId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ProjectId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LayerId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkId {
    pub x: u32,
    pub y: u32,
    pub level: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_round_trips_as_hex() {
        let id = ProjectId::generate(42, 1_700_000_000);
        let text = id.to_string();
        assert_eq!(text.len(), 32);
        assert_eq!(text.parse::<ProjectId>().unwrap(), id);
    }
}
