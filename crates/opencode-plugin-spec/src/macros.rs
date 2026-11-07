//! The two shapes the generated code repeats: string enums and records.

/// A record the host decodes from, or encodes to, a Luau table through serde.
pub trait Record {
    /// The contract's name for it.
    const NAME: &'static str;
    /// Its data keys, as Luau spells them.
    const FIELDS: &'static [&'static str];
    /// Keys that hold Luau functions. Serde skips them; the host keeps them as references.
    const CALLBACKS: &'static [&'static str];
    /// Keys that hold host objects. Serde skips them; the host attaches them itself.
    const HANDLES: &'static [&'static str];

    /// Whether `key` belongs to the record. The host warns once per unknown key and ignores it.
    fn knows(key: &str) -> bool {
        Self::FIELDS.contains(&key) || Self::CALLBACKS.contains(&key) || Self::HANDLES.contains(&key)
    }
}

/// Implements [`Record`]: `impl_record!(Name, [fields], [callbacks], [handles])`.
#[macro_export]
#[doc(hidden)]
macro_rules! impl_record {
    ($name:ident, [$($field:literal),*], [$($callback:literal),*], [$($handle:literal),*]) => {
        impl $crate::Record for $name {
            const NAME: &'static str = stringify!($name);
            const FIELDS: &'static [&'static str] = &[$($field),*];
            const CALLBACKS: &'static [&'static str] = &[$($callback),*];
            const HANDLES: &'static [&'static str] = &[$($handle),*];
        }
    };
}

/// An enum of fixed strings: `parse`, `as_str`, `ALL`, serde as the string, and the first
/// variant as the default.
#[macro_export]
#[doc(hidden)]
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(#[$first_meta:meta])*
            $first:ident = $first_value:literal,
            $(
                $(#[$variant_meta:meta])*
                $variant:ident = $value:literal,
            )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $(#[$first_meta])*
            #[default]
            $first,
            $(
                $(#[$variant_meta])*
                $variant,
            )*
        }

        impl $name {
            /// Every value, in contract order.
            pub const ALL: &'static [$name] = &[$name::$first, $($name::$variant),*];

            /// The contract's string.
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_value,
                    $($name::$variant => $value,)*
                }
            }

            /// The value for a contract string.
            #[allow(clippy::should_implement_trait)]
            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $first_value => Some($name::$first),
                    $($value => Some($name::$variant),)*
                    _ => None,
                }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = <::std::borrow::Cow<'de, str> as ::serde::Deserialize>::deserialize(deserializer)?;
                $name::parse(&s).ok_or_else(|| {
                    let expected: Vec<&str> = $name::ALL.iter().map(|v| v.as_str()).collect();
                    ::serde::de::Error::custom(format_args!("unknown {} `{}`, expected one of: {}", stringify!($name), s, expected.join(", ")))
                })
            }
        }
    };
}
