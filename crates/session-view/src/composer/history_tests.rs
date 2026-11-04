use rune::TextEdit;

use super::{Entry, PromptHistory};

#[test]
fn recalls_prompts_newest_first_and_restores_draft() {
    let mut h = PromptHistory::<String>::default();
    let scope = "ses_a".to_owned();
    assert!(h.add(Entry { text: "first".into(), pills: Vec::new() }, false));
    assert!(h.add(Entry { text: "second".into(), pills: Vec::new() }, false));
    assert!(!h.add(Entry { text: "second".into(), pills: Vec::new() }, false));

    let mut c = TextEdit::new("");
    assert!(h.step(&mut c, &scope, false, true));
    assert_eq!((c.text.as_str(), c.focus), ("second", 0));
    assert!(h.step(&mut c, &scope, false, true));
    assert_eq!((c.text.as_str(), c.focus), ("first", 0));
    assert!(!h.step(&mut c, &scope, false, true));

    assert!(h.step(&mut c, &scope, false, false));
    assert_eq!((c.text.as_str(), c.focus), ("second", 6));
    assert!(h.step(&mut c, &scope, false, false));
    assert_eq!((c.text.as_str(), c.focus), ("", 0));
}
