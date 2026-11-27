//! Seed-based variants use the same V1/V2 token bridge as the bundled themes.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{Map, Value};

#[path = "color.rs"]
mod color;
#[path = "legacy.rs"]
mod legacy;
#[path = "mapping.rs"]
mod mapping;

pub struct Seeds<'a> {
    pub neutral: &'a str,
    pub primary: &'a str,
    pub success: &'a str,
    pub warning: &'a str,
    pub error: &'a str,
    pub info: &'a str,
    pub interactive: &'a str,
    pub diff_add: &'a str,
    pub diff_delete: &'a str,
}

type Tokens = BTreeMap<String, String>;

// The per-scheme defaults are the seeds of OpenCode's built-in oc-2 palette.
const DEFAULTS: [[&str; 9]; 2] = [
    [
        "#f7f7f7", "#dcde8d", "#12c905", "#ffdc17", "#fc533a", "#a753ae", "#034cff", "#9ff29a",
        "#fc533a",
    ],
    [
        "#1f1f1f", "#fab283", "#12c905", "#fcd53a", "#fc533a", "#edb2f1", "#034cff", "#c8ffc4",
        "#fc533a",
    ],
];

impl<'a> From<&'a [String; 9]> for Seeds<'a> {
    fn from(
        [
            neutral,
            primary,
            success,
            warning,
            error,
            info,
            interactive,
            diff_add,
            diff_delete,
        ]: &'a [String; 9],
    ) -> Self {
        Self {
            neutral,
            primary,
            success,
            warning,
            error,
            info,
            interactive,
            diff_add,
            diff_delete,
        }
    }
}

pub fn resolve_variant(
    seeds: &Seeds<'_>,
    dark: bool,
    overrides: Option<&BTreeMap<String, String>>,
) -> Map<String, Value> {
    let inputs = [
        seeds.neutral,
        seeds.primary,
        seeds.success,
        seeds.warning,
        seeds.error,
        seeds.info,
        seeds.interactive,
        seeds.diff_add,
        seeds.diff_delete,
    ];
    let valid: [String; 9] = std::array::from_fn(|i| {
        hex(inputs[i])
            .filter(|color| &color[7..] == "ff")
            .map_or_else(
                || DEFAULTS[usize::from(dark)][i].to_owned(),
                |color| color[..7].to_owned(),
            )
    });
    let seeds = Seeds::from(&valid);
    let mut tokens = v2(&seeds, dark);
    tokens.extend(
        legacy::resolve(&seeds, dark)
            .into_iter()
            .filter_map(|(key, value)| Some((key, hex(&value)?))),
    );
    if let Some(overrides) = overrides {
        tokens.extend(overrides.iter().filter_map(|(key, value)| {
            Some((
                key.clone(),
                hex(value).or_else(|| palette_hex(value.trim()))?,
            ))
        }));
    }
    tokens
        .into_iter()
        .map(|(key, value)| (key, Value::String(value)))
        .collect()
}

fn palette_hex(key: &str) -> Option<String> {
    static PALETTE: OnceLock<Value> = OnceLock::new();
    let palette =
        PALETTE.get_or_init(|| serde_json::from_str(crate::palette::THEME).unwrap_or_default());
    hex(palette["palette"][key].as_str()?)
}

fn hex(value: &str) -> Option<String> {
    let digits = value.trim().strip_prefix('#')?;
    if !digits.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digits = digits.to_ascii_lowercase();
    match digits.len() {
        3 => {
            let rgb: String = digits.chars().flat_map(|c| [c, c]).collect();
            Some(format!("#{rgb}ff"))
        }
        6 => Some(format!("#{digits}ff")),
        8 => Some(format!("#{digits}")),
        _ => None,
    }
}

// OpenCode packages/ui/src/theme/v2/resolve.ts at 5050225a097af2a3ffd5c9b6e39d53ba3d69eeec.
fn hue(seed: &str, dark: bool) -> [String; 12] {
    let base = color::hex_to_oklch(seed);
    let l = base.l;
    let steps = [
        0.99,
        0.965,
        0.93,
        0.885,
        0.835,
        l.clamp(0.48, 0.72),
        (l - 0.07).clamp(0.4, 0.64),
        (l - 0.14).clamp(0.32, 0.55),
        (l - 0.21).clamp(0.24, 0.46),
        (l - 0.28).clamp(0.17, 0.38),
        (l - 0.34).clamp(0.12, 0.3),
        (l - 0.4).clamp(0.08, 0.22),
    ];
    let chroma = [
        0.28, 0.48, 0.68, 0.86, 1.02, 1.28, 1.34, 1.28, 1.18, 1.08, 0.98, 0.88,
    ];
    let boost = if dark { 1.0 } else { 1.05 };
    std::array::from_fn(|i| {
        color::oklch_to_hex(color::Oklch {
            l: steps[i],
            c: base.c * chroma[i] * boost,
            h: base.h,
        })
    })
}

fn v2(seeds: &Seeds<'_>, dark: bool) -> Tokens {
    let mut grey = color::generate_neutral_scale(seeds.neutral, dark, Some(seeds.neutral));
    if dark {
        grey.reverse();
    }
    let mut tokens = mapping::semantics(dark);
    tokens.insert("grey-50".into(), "#ffffffff".into());
    for (name, ramp) in [
        ("grey", grey),
        ("blue", hue(seeds.interactive, dark)),
        ("green", hue(seeds.success, dark)),
        ("yellow", hue(seeds.warning, dark)),
        ("red", hue(seeds.error, dark)),
        ("purple", hue(seeds.info, dark)),
        ("pink", hue(seeds.info, dark)),
        (
            "orange",
            hue(&color::shift(seeds.warning, -0.082, 0.94, -22.0), dark),
        ),
        (
            "cyan",
            hue(&color::shift(seeds.info, 0.128, 1.12, -12.0), dark),
        ),
    ] {
        tokens.extend(
            ramp.into_iter()
                .enumerate()
                .map(|(i, value)| (format!("{name}-{}", (i + 1) * 100), value)),
        );
    }
    tokens.extend(foreground(seeds.neutral, dark, &tokens));
    mapping::V2_KEYS
        .iter()
        .filter_map(|&key| Some((key.to_owned(), flatten(&tokens, key)?)))
        .collect()
}

fn foreground(ink: &str, dark: bool, tokens: &Tokens) -> Tokens {
    let l = color::hex_to_oklch(ink).l;
    let body = if dark {
        color::shift(ink, (0.88 - l).max(0.0) * 0.4, 1.04, 0.0)
    } else {
        color::shift(ink, -(l - 0.18).max(0.0) * 0.24, 1.02, 0.0)
    };
    let pick = |background, ratio, target: i32| {
        let bg = flatten(tokens, background);
        let step = (1..=12)
            .map(|i| i * 100)
            .filter(|i| {
                bg.as_ref()
                    .zip(tokens.get(&format!("grey-{i}")))
                    .is_some_and(|(bg, fg)| color::contrast_ratio(fg, &bg[..7]) >= ratio)
            })
            .min_by_key(|i| (i - target).abs())
            .unwrap_or(target);
        format!("var(--v2-grey-{step})")
    };
    let base = "background-bg-base";
    let inverse = "background-bg-inverse";
    let inverse_target = if flatten(tokens, inverse)
        .is_some_and(|color| color::hex_to_oklch(&color[..7]).l > 0.55)
    {
        1100
    } else {
        50
    };
    [
        (
            "text-text-base",
            if dark {
                color::blend("#ffffff", &body, 0.9)
            } else {
                color::shift(&body, -0.07, 1.04, 0.0)
            },
        ),
        (
            "text-text-muted",
            color::shift(&body, if dark { -0.11 } else { 0.11 }, 0.9, 0.0),
        ),
        (
            "text-text-faint",
            color::shift(
                &body,
                if dark { -0.2 } else { 0.21 },
                if dark { 0.78 } else { 0.72 },
                0.0,
            ),
        ),
        (
            "icon-icon-base",
            pick(base, 7.0, if dark { 400 } else { 800 }),
        ),
        ("icon-icon-muted", pick(base, 3.0, 600)),
        ("icon-icon-faint", pick(base, 1.5, 500)),
        ("icon-icon-inverse", pick(inverse, 7.0, inverse_target)),
        (
            "icon-icon-contrast",
            pick("background-bg-contrast", 7.0, 100),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

fn flatten<'a>(tokens: &'a Tokens, mut key: &'a str) -> Option<String> {
    for _ in 0..tokens.len() {
        if key.starts_with("alpha-") {
            return palette_hex(key);
        }
        let value = tokens.get(key)?;
        match value
            .strip_prefix("var(--v2-")
            .and_then(|value| value.strip_suffix(')'))
        {
            Some(reference) => key = reference,
            None => return hex(value),
        }
    }
    None
}
