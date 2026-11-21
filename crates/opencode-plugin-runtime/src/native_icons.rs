//! Desktop's icons the core panels draw that the theme's set doesn't name: the review toolbar's
//! file arrows and its segmented controls' marks (`review-arrow-left`, `review-arrow-right`,
//! `review-unified`, `review-split`, `review-expand-lines`, `review-collapse-lines`,
//! `select-chevron`), under the names plugins pass to `ui.icon` and `ui.segmented`.

use rune::Icon;

static ARROW_LEFT: Icon = Icon {
    name: "review-arrow-left",
    view: 16.0,
    body: r#"<g transform="rotate(270 8 8)"><path d="M8 12.5V3.5M11.5 7L8 3.5L4.5 7" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" fill="none"/></g>"#,
};
static ARROW_RIGHT: Icon = Icon {
    name: "review-arrow-right",
    view: 16.0,
    body: r#"<g transform="rotate(90 8 8)"><path d="M8 12.5V3.5M11.5 7L8 3.5L4.5 7" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" fill="none"/></g>"#,
};
static UNIFIED: Icon = Icon {
    name: "review-unified",
    view: 16.0,
    body: r#"<path d="M14.5 13.5H1.5V2.5H14.5V13.5Z" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" fill="none"/><rect opacity="0.5" x="3" y="4" width="10" height="3" fill="currentColor"/><rect opacity="0.5" x="3" y="9" width="10" height="3" fill="currentColor"/>"#,
};
static SPLIT: Icon = Icon {
    name: "review-split",
    view: 16.0,
    body: r#"<path d="M14.5 13.5H1.5V2.5H14.5V13.5Z" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" fill="none"/><rect opacity="0.5" x="3" y="4" width="4" height="8" fill="currentColor"/><rect opacity="0.5" x="9" y="4" width="4" height="8" fill="currentColor"/>"#,
};
static EXPAND_LINES: Icon = Icon {
    name: "review-expand-lines",
    view: 16.0,
    body: r#"<path d="M10.5 4.5L8 2L5.5 4.5M10.5 11.5L8 14L5.5 11.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" fill="none"/><path d="M2.5 8H13.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static COLLAPSE_LINES: Icon = Icon {
    name: "review-collapse-lines",
    view: 16.0,
    body: r#"<path d="M5.5 2.5L8 5L10.5 2.5M5.5 13.5L8 11L10.5 13.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" fill="none"/><path d="M2.5 8H13.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
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
