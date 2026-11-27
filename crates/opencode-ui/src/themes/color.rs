// Ported from the pinned OpenCode theme reference `packages/ui/src/theme/color.ts`. Colours
// come back as lowercase `#rrggbb`, or `#rrggbbaa` from `with_alpha`.

#[derive(Clone, Copy)]
pub(super) struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

fn clamp(v: f64, min: f64, max: f64) -> f64 {
    v.max(min).min(max)
}

fn hue(v: f64) -> f64 {
    ((v % 360.0) + 360.0) % 360.0
}

fn hex_to_rgb(hex: &str) -> (f64, f64, f64) {
    let h = hex.replace('#', "");

    let full = if h.len() == 3 || h.len() == 4 {
        h.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        h
    };

    let rgb = if full.len() == 8 {
        &full[..6]
    } else {
        full.as_str()
    };

    let num = u32::from_str_radix(rgb, 16).unwrap_or_default();

    (
        f64::from((num >> 16) & 255) / 255.0,
        f64::from((num >> 8) & 255) / 255.0,
        f64::from(num & 255) / 255.0,
    )
}

fn rgb_to_hex(r: f64, g: f64, b: f64) -> String {
    let to_hex = |v: f64| format!("{:02x}", (clamp(v, 0.0, 1.0) * 255.0).round() as u32);

    format!("#{}{}{}", to_hex(r), to_hex(g), to_hex(b))
}

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        return c * 12.92;
    }

    1.055 * c.powf(1.0 / 2.4) - 0.055
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        return c / 12.92;
    }

    ((c + 0.055) / 1.055).powf(2.4)
}

fn rgb_to_oklch(r: f64, g: f64, b: f64) -> Oklch {
    let lr = srgb_to_linear(r);
    let lg = srgb_to_linear(g);
    let lb = srgb_to_linear(b);

    let l_ = 0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb;
    let m_ = 0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb;
    let s_ = 0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb;

    let l = l_.cbrt();
    let m = m_.cbrt();
    let s = s_.cbrt();

    let lightness = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
    let b_ok = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;

    let c = (a * a + b_ok * b_ok).sqrt();
    let mut h = b_ok.atan2(a) * (180.0 / std::f64::consts::PI);

    if h < 0.0 {
        h += 360.0;
    }

    Oklch { l: lightness, c, h }
}

fn oklch_to_rgb(oklch: Oklch) -> (f64, f64, f64) {
    let Oklch { l, c, h } = oklch;

    let a = c * (h * std::f64::consts::PI / 180.0).cos();
    let b = c * (h * std::f64::consts::PI / 180.0).sin();

    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.291485548 * b;

    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;

    let lr = 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3;
    let lg = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3;
    let lb = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.707614701 * s3;

    (linear_to_srgb(lr), linear_to_srgb(lg), linear_to_srgb(lb))
}

pub(super) fn hex_to_oklch(hex: &str) -> Oklch {
    let (r, g, b) = hex_to_rgb(hex);

    rgb_to_oklch(r, g, b)
}

fn fit_oklch(oklch: Oklch) -> Oklch {
    let base = Oklch {
        l: clamp(oklch.l, 0.0, 1.0),
        c: oklch.c.max(0.0),
        h: hue(oklch.h),
    };

    // Every channel has to land inside sRGB, so hold the hue and lightness and pull chroma in
    // until it does; the reference gives up at zero chroma after 24 steps.
    let (r, g, b) = oklch_to_rgb(base);

    if (0.0..=1.0).contains(&r) && (0.0..=1.0).contains(&g) && (0.0..=1.0).contains(&b) {
        return base;
    }

    let mut c = base.c;

    for _ in 0..24 {
        c *= 0.9;
        let next = Oklch { c, ..base };
        let (r, g, b) = oklch_to_rgb(next);

        if (0.0..=1.0).contains(&r) && (0.0..=1.0).contains(&g) && (0.0..=1.0).contains(&b) {
            return next;
        }
    }

    Oklch { c: 0.0, ..base }
}

pub(super) fn oklch_to_hex(color: Oklch) -> String {
    let (r, g, b) = oklch_to_rgb(fit_oklch(color));

    rgb_to_hex(r, g, b)
}

pub(super) fn generate_scale(seed: &str, dark: bool) -> [String; 12] {
    let base = hex_to_oklch(seed);

    let light_steps: [f64; 12] = if dark {
        [
            0.118,
            0.138,
            0.167,
            0.202,
            0.246,
            0.304,
            0.378,
            0.468,
            clamp(base.l * 0.825, 0.53, 0.705),
            clamp(base.l * 0.89, 0.61, 0.79),
            clamp(base.l + 0.033, 0.868, 0.943),
            0.984,
        ]
    } else {
        [
            0.993,
            0.983,
            0.962,
            0.936,
            0.906,
            0.866,
            0.811,
            0.74,
            base.l,
            (base.l - 0.036).max(0.0),
            0.49,
            0.27,
        ]
    };

    let chroma_multipliers: [f64; 12] = if dark {
        [
            0.52, 0.68, 0.86, 1.02, 1.14, 1.24, 1.36, 1.48, 1.56, 1.64, 1.62, 1.15,
        ]
    } else {
        [
            0.12, 0.24, 0.46, 0.68, 0.84, 0.98, 1.08, 1.16, 1.22, 1.26, 1.18, 0.98,
        ]
    };

    std::array::from_fn(|i| {
        oklch_to_hex(Oklch {
            l: light_steps[i],
            c: base.c * chroma_multipliers[i],
            h: base.h,
        })
    })
}

pub(super) fn generate_neutral_scale(seed: &str, dark: bool, ink: Option<&str>) -> [String; 12] {
    if let Some(ink) = ink {
        let base = hex_to_oklch(seed);

        let lift = |tone: f64| {
            oklch_to_hex(Oklch {
                l: base.l + (1.0 - base.l) * tone,
                c: base.c * (1.0 - tone).max(0.0),
                h: base.h,
            })
        };

        let sink = |tone: f64| {
            let keep = 1.0 - tone * if dark { 0.12 } else { 0.3 };
            oklch_to_hex(Oklch {
                l: base.l * (1.0 - tone),
                c: base.c * keep.max(0.0),
                h: base.h,
            })
        };

        // The ground sits under the ink, lifted towards it on light or sunk away from it on
        // dark, by how much colour the seed carries.
        let bg = if dark {
            sink(clamp(
                0.19 + (base.l - 0.12).max(0.0) * 0.33 + base.c * 1.95,
                0.17,
                0.27,
            ))
        } else if base.l < 0.82 {
            lift(0.86)
        } else {
            lift(clamp(
                0.1 + base.c * 3.2 + (0.95 - base.l).max(0.0) * 0.35,
                0.1,
                0.28,
            ))
        };

        let steps: [f64; 12] = if dark {
            [
                0.0, 0.018, 0.039, 0.064, 0.097, 0.143, 0.212, 0.31, 0.46, 0.649, 0.845, 0.984,
            ]
        } else {
            [
                0.0, 0.022, 0.042, 0.068, 0.102, 0.146, 0.208, 0.296, 0.432, 0.61, 0.81, 0.965,
            ]
        };

        return steps.map(|step| mix_colors(&bg, ink, step));
    }

    let base = hex_to_oklch(seed);
    let neutral_chroma = base.c.min(if dark { 0.068 } else { 0.04 });

    let light_steps: [f64; 12] = if dark {
        [
            0.138,
            0.156,
            0.178,
            0.202,
            0.232,
            0.272,
            0.326,
            0.404,
            clamp(base.l * 0.83, 0.43, 0.55),
            0.596,
            0.719,
            0.956,
        ]
    } else {
        [
            0.991, 0.979, 0.964, 0.946, 0.931, 0.913, 0.891, 0.83, base.l, 0.617, 0.542, 0.205,
        ]
    };

    std::array::from_fn(|i| {
        oklch_to_hex(Oklch {
            l: light_steps[i],
            c: neutral_chroma,
            h: base.h,
        })
    })
}

fn mix_colors(color1: &str, color2: &str, amount: f64) -> String {
    let c1 = hex_to_oklch(color1);
    let c2 = hex_to_oklch(color2);
    // The shorter way round the circle, so a mix never takes the long way through the hues.
    let delta = ((((c2.h - c1.h) % 360.0) + 540.0) % 360.0) - 180.0;

    oklch_to_hex(Oklch {
        l: c1.l + (c2.l - c1.l) * amount,
        c: c1.c + (c2.c - c1.c) * amount,
        h: c1.h + delta * amount,
    })
}

pub(super) fn shift(hex: &str, l: f64, c: f64, h: f64) -> String {
    let base = hex_to_oklch(hex);

    oklch_to_hex(Oklch {
        l: base.l + l,
        c: base.c * c,
        h: base.h + h,
    })
}

pub(super) fn contrast_ratio(foreground: &str, background: &str) -> f64 {
    let linear = |c: f64| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };

    let luminance = |hex: &str| {
        let (r, g, b) = hex_to_rgb(hex);

        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    };

    let fg = luminance(foreground);
    let bg = luminance(background);
    let lighter = fg.max(bg);
    let darker = fg.min(bg);

    (lighter + 0.05) / (darker + 0.05)
}

pub(super) fn blend(color: &str, background: &str, alpha: f64) -> String {
    let fg = hex_to_rgb(color);
    let bg = hex_to_rgb(background);

    rgb_to_hex(
        fg.0 * alpha + bg.0 * (1.0 - alpha),
        fg.1 * alpha + bg.1 * (1.0 - alpha),
        fg.2 * alpha + bg.2 * (1.0 - alpha),
    )
}
