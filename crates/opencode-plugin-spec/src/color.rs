//! A `Color` value: a semantic token, or a literal `#RRGGBB[AA]` from `ui.hex`.

use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::SemanticToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorSpec {
    /// Follows the theme.
    Token(SemanticToken),
    /// Straight-alpha RGBA.
    Rgba([u8; 4]),
}

impl Default for ColorSpec {
    fn default() -> Self {
        ColorSpec::Token(SemanticToken::TextBase)
    }
}

impl ColorSpec {
    /// A token name, or `#RGB`, `#RGBA`, `#RRGGBB` or `#RRGGBBAA`.
    pub fn parse(s: &str) -> Option<Self> {
        let Some(hex) = s.strip_prefix('#') else {
            return SemanticToken::parse(s).map(ColorSpec::Token);
        };
        let digit = |i: usize| hex.get(i..i + 1).and_then(|d| u8::from_str_radix(d, 16).ok());
        let byte = |i: usize| hex.get(i..i + 2).and_then(|d| u8::from_str_radix(d, 16).ok());
        let rgba = match hex.len() {
            3 | 4 => {
                let short = |i: usize| digit(i).map(|d| d * 17);
                [short(0)?, short(1)?, short(2)?, if hex.len() == 4 { short(3)? } else { 255 }]
            }
            6 | 8 => [byte(0)?, byte(2)?, byte(4)?, if hex.len() == 8 { byte(6)? } else { 255 }],
            _ => return None,
        };
        Some(ColorSpec::Rgba(rgba))
    }
}

impl std::fmt::Display for ColorSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColorSpec::Token(t) => f.write_str(t.as_str()),
            ColorSpec::Rgba([r, g, b, a]) => write!(f, "#{r:02x}{g:02x}{b:02x}{a:02x}"),
        }
    }
}

impl Serialize for ColorSpec {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ColorSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        ColorSpec::parse(&s).ok_or_else(|| serde::de::Error::custom(format_args!("`{s}` is neither a semantic token nor #RRGGBB[AA]")))
    }
}
