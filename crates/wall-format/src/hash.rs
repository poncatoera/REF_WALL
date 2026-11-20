use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::Error;

/// BLAKE3 hash of a media file's bytes, as 64 lowercase hex characters.
/// It names the file on disk and in R2, which makes dedupe and sync trivial.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentHash(String);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes).to_hex().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// First two hex characters, used as a fan-out folder (`media/ab/…`).
    pub fn shard(&self) -> &str {
        &self.0[..2]
    }
}

impl FromStr for ContentHash {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let ok = s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if ok { Ok(Self(s.to_owned())) } else { Err(Error::BadHash(s.to_owned())) }
    }
}

impl TryFrom<String> for ContentHash {
    type Error = Error;
    fn try_from(s: String) -> Result<Self, Error> {
        s.parse()
    }
}

impl From<ContentHash> for String {
    fn from(h: ContentHash) -> String {
        h.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({}…)", &self.0[..12])
    }
}
