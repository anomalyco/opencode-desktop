//! Desktop's icons the core panels draw that the theme's set doesn't name: the review toolbar's
//! file arrows and its segmented controls' marks (`arrow-left`, `arrow-right`, `unified`,
//! `split`, `expand`, `collapse`), under the names plugins pass to `ui.icon` and `ui.segmented`.

use rune::Icon;

static ARROW_LEFT: Icon = Icon {
    name: "review-arrow-left",
    view: 20.0,
    body: r#"<path d="M8.33464 4.58398L2.91797 10.0007L8.33464 15.4173M3.33464 10.0007H17.0846" stroke="currentColor" stroke-linecap="square" fill="none"/>"#,
};
static ARROW_RIGHT: Icon = Icon {
    name: "review-arrow-right",
    view: 20.0,
    body: r#"<path d="M11.6654 4.58398L17.082 10.0007L11.6654 15.4173M16.6654 10.0007H2.91536" stroke="currentColor" stroke-linecap="square" fill="none"/>"#,
};
static UNIFIED: Icon = Icon {
    name: "review-unified",
    view: 16.0,
    body: r#"<path d="M3.00001 4.00045L12.9998 4L13 6.99955L3 7L3.00001 4.00045Z" fill="currentColor" fill-opacity="0.5"/><path d="M3.0001 9H13L12.9999 12H3L3.0001 9Z" fill="currentColor" fill-opacity="0.5"/><path d="M1 14H15L15 2H1V14Z" stroke="currentColor" fill="none"/>"#,
};
static SPLIT: Icon = Icon {
    name: "review-split",
    view: 16.0,
    body: r#"<path d="M1 14H15L15 2H1V14Z" stroke="currentColor" fill="none"/><rect x="3" y="4" width="4" height="8" fill="currentColor" fill-opacity="0.5"/><rect x="9" y="4" width="4" height="8" fill="currentColor" fill-opacity="0.5"/>"#,
};
static EXPAND_LINES: Icon = Icon {
    name: "review-expand-lines",
    view: 16.0,
    body: r#"<path d="M8.25 6.17773V1.17773M11.25 4.17773L8.25 1.17773L5.25 4.17773" stroke="currentColor" fill="none"/><path d="M8.25 9.17773V14.1777M11.25 11.1777L8.25 14.1777L5.25 11.1777" stroke="currentColor" fill="none"/><path d="M4.25 7.67773H12.25" stroke="currentColor"/>"#,
};
static COLLAPSE_LINES: Icon = Icon {
    name: "review-collapse-lines",
    view: 16.0,
    body: r#"<path d="M8 1V6M11 3L8 6L5 3" stroke="currentColor" fill="none"/><path d="M8 15V10M11 13L8 10L5 13" stroke="currentColor" fill="none"/><path d="M4 8H6M7 8H9M10 8H12" stroke="currentColor"/>"#,
};

/// select-v2's chevron (`ChevronDown`, turned to point down).
static SELECT_CHEVRON: Icon = Icon {
    name: "select-chevron",
    view: 16.0,
    body: r#"<path d="M5 6.5L8 9.5L11 6.5" stroke="currentColor" stroke-width="1" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

const ALL: [&Icon; 7] = [&ARROW_LEFT, &ARROW_RIGHT, &UNIFIED, &SPLIT, &EXPAND_LINES, &COLLAPSE_LINES, &SELECT_CHEVRON];

/// The icon named `name`, if it is one of these.
pub fn named(name: &str) -> Option<&'static Icon> {
    ALL.iter().copied().find(|i| i.name == name)
}
