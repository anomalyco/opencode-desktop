// Ported from the pinned OpenCode V1 resolver `packages/ui/src/theme/resolve.ts` at
// 5050225a097af2a3ffd5c9b6e39d53ba3d69eeec: the tokens of its seed-only (`compact: false`)
// branch that the bridge exports, as literal hex. Overrides are the caller's, laid on last.
use super::color::{blend, generate_neutral_scale, generate_scale};
use super::{Seeds, Tokens};

pub(super) fn resolve(seeds: &Seeds<'_>, dark: bool) -> Tokens {
    let neutral = generate_neutral_scale(seeds.neutral, dark, None);
    let interactive = generate_scale(seeds.interactive, dark);
    let diff_add = generate_scale(seeds.diff_add, dark);
    let diff_delete = generate_scale(seeds.diff_delete, dark);

    // The bridge resolves the code colours' `var(--text-*)` references, so those aliases come
    // back as the same hex.
    let text_base = &neutral[10];
    let text_weak = &neutral[8];
    let text_strong = &neutral[11];
    let weak_or_base = pick(dark, text_weak, text_base);
    let stronger = pick(dark, neutral[1].as_str(), "#fcfcfc").to_owned();
    let raised = pick(dark, neutral[2].as_str(), "#ffffff").to_owned();
    let icon = neutral[if dark { 9 } else { 8 }].as_str();
    let interactive_base = interactive[8].clone();
    let hover = blend(&neutral[11], &neutral[0], if dark { 0.066 } else { 0.06 });
    let string = pick(dark, "#00ceb9", "#006656");
    let primitive = pick(dark, "#ffba92", "#fb4804");
    let property = pick(dark, "#ff9ae2", "#ed6dc8");
    let ty = pick(dark, "#ecf58c", "#596600");
    let constant = pick(dark, "#93e9f6", "#007b80");
    let info = pick(dark, "#93e9f6", "#0092a8");

    let mut tokens = Tokens::new();
    tokens.insert("background-stronger".to_owned(), stronger);
    tokens.insert("icon-base".to_owned(), icon.to_owned());
    tokens.insert("icon-interactive-base".to_owned(), interactive_base);
    tokens.insert("surface-raised-base-hover".to_owned(), hover);
    tokens.insert("surface-raised-stronger-non-alpha".to_owned(), raised);
    tokens.insert("syntax-comment".to_owned(), text_weak.to_owned());
    tokens.insert("syntax-constant".to_owned(), constant.to_owned());
    tokens.insert("syntax-diff-add".to_owned(), diff_add[10].clone());
    tokens.insert("syntax-diff-delete".to_owned(), diff_delete[10].clone());
    tokens.insert("syntax-info".to_owned(), info.to_owned());
    tokens.insert("syntax-keyword".to_owned(), text_weak.to_owned());
    tokens.insert("syntax-operator".to_owned(), weak_or_base.to_owned());
    tokens.insert("syntax-primitive".to_owned(), primitive.to_owned());
    tokens.insert("syntax-property".to_owned(), property.to_owned());
    tokens.insert("syntax-punctuation".to_owned(), weak_or_base.to_owned());
    tokens.insert("syntax-regexp".to_owned(), text_base.to_owned());
    tokens.insert("syntax-string".to_owned(), string.to_owned());
    tokens.insert("syntax-type".to_owned(), ty.to_owned());
    tokens.insert("syntax-variable".to_owned(), text_strong.to_owned());
    tokens.insert("text-base".to_owned(), text_base.to_owned());
    tokens.insert("text-strong".to_owned(), text_strong.to_owned());
    tokens.insert("text-stronger".to_owned(), text_strong.to_owned());
    tokens.insert("text-weak".to_owned(), text_weak.to_owned());
    tokens
}

// `if dark` picks `dark_value`, matching the resolver's ternary branches.
fn pick<'a>(dark: bool, dark_value: &'a str, light_value: &'a str) -> &'a str {
    if dark { dark_value } else { light_value }
}
