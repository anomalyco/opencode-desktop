use crate::*;

fn msgs(src: &str) -> Vec<String> {
    check_outline(src, &BudgetProfile::CARD).iter().map(ToString::to_string).collect()
}

#[test]
fn every_example_checks_clean() {
    for s in CATALOG {
        assert_eq!(msgs(s.example), Vec::<String>::new(), "{}'s example", s.name);
        let used = |ns: &[Node]| -> bool {
            fn any(ns: &[Node], name: &str) -> bool {
                ns.iter().any(|n| n.part.name == name || any(&n.kids, name))
            }
            any(ns, s.name)
        };
        assert!(used(&parse_outline(s.example)), "{}'s example doesn't use it", s.name);
        assert!(s.about.split_whitespace().count() <= 10, "{}'s about is over ten words", s.name);
    }
}

#[test]
fn lines_split_the_way_their_part_asks() {
    let n = parse_outline("say Kit {voice}: filter Seer to Hex\nkv Status: {proposal} Nothing built · yet\nitem {done} Parser · landed\ntoggle {on} Skip the mic\nchips a · b, c");
    let p: Vec<&Part> = n.iter().map(|n| &n.part).collect();
    assert_eq!((p[0].key.as_str(), p[0].tone.as_deref(), p[0].text.as_str()), ("Kit", Some("voice"), "filter Seer to Hex"));
    assert_eq!((p[1].key.as_str(), p[1].tone.as_deref(), p[1].text.as_str(), p[1].detail.as_str()), ("Status", Some("proposal"), "Nothing built", "yet"));
    assert_eq!((p[2].tone.as_deref(), p[2].text.as_str(), p[2].detail.as_str()), (Some("done"), "Parser", "landed"));
    assert_eq!((p[3].tone.as_deref(), p[3].text.as_str()), (Some("on"), "Skip the mic"));
    assert_eq!(p[4].text, "a · b, c");
}

#[test]
fn kids_nest_by_indent_and_keep_their_line() {
    let n = parse_outline("row\n  box Today\n    item {risk} Pasted\n\n  arrow\n  box After");
    assert_eq!(n.len(), 1);
    let kids: Vec<_> = n[0].kids.iter().map(|k| (k.part.name.as_str(), k.key())).collect();
    assert_eq!(kids, [("box", "L2".into()), ("arrow", "L5".into()), ("box", "L6".into())]);
    assert_eq!(n[0].kids[0].kids[0].part.text, "Pasted");
}

#[test]
fn mistakes_name_the_fix() {
    assert_eq!(msgs("box\n  buton Ship"), ["line 2: unknown part `buton`; did you mean `button`?"]);
    assert_eq!(msgs("screen\n  said Kit: hi"), ["line 2: unknown part `said`; did you mean `say`?"]);
    assert!(msgs("zebra")[0].contains("parts: row, col"));
    assert_eq!(msgs("item {done} A\n  text B"), ["line 1: `item` takes nothing under it; put the indented lines in a `box`"]);
    assert_eq!(msgs("screen\n  say Kit: hi\n    text no"), ["line 3: `say` takes only `tools` under it"]);
    assert_eq!(msgs("box\n  toggle Skip"), ["line 2: `toggle` needs {on} or {off} before its label"]);
    assert_eq!(msgs("box\n   text Odd"), ["line 2: indent by two spaces a level"]);
    assert_eq!(msgs("box\n    text Deep"), ["line 2: indent two spaces under `box` (line 1)"]);
    assert_eq!(msgs(""), ["line 1: the ui fence is empty"]);
}

#[test]
fn budgets_hold_where_the_outline_draws() {
    let deep = "box\n  box\n    box\n      box\n        text Four deep";
    assert_eq!(msgs(deep), ["line 4: frames nested 4 deep; the most is 3"]);
    assert!(check_outline(deep, &BudgetProfile::CHAT).is_empty());
    let long = format!("box\n  text {}", "word ".repeat(23));
    assert_eq!(msgs(&long), ["line 2: 23 words on one line; the most is 22"]);
}

#[test]
fn a_note_counts_prose_and_outline_words_together() {
    let note = format!("{}\n\n```ui\nbox\n  text {}\n```\n\n```rust\n{}\n```\n", "word ".repeat(140), "w ".repeat(12), "code ".repeat(100));
    let issues = check(&note, &BudgetProfile::CARD);
    assert_eq!(issues.iter().map(ToString::to_string).collect::<Vec<_>>(), ["line 1: 152 words; a card holds 150. Cut prose or outline lines"]);
    let bad = "Intro\n\n```ui\nbox\n  buton Ship\n```\n";
    assert_eq!(check(bad, &BudgetProfile::CARD)[0].to_string(), "line 5: unknown part `buton`; did you mean `button`?");
}

#[test]
fn streaming_commits_whole_lines_and_draws_an_open_fence() {
    let src = "Plan:\n\n```ui\nrow\n  box Today\n  box Aft";
    let b = parse_note(src, true);
    assert_eq!(b.len(), 2);
    let Block::Ui { nodes, open, line, .. } = &b[1] else { panic!("{b:?}") };
    assert!(*open);
    assert_eq!(*line, 3);
    assert_eq!(nodes[0].kids.len(), 1, "the half line waits");
    let Block::Ui { nodes, .. } = &parse_note(src, false)[1] else { panic!() };
    assert_eq!(nodes[0].kids.len(), 2);
    assert_eq!(parse_note("Hello wor", true), [Block::Prose { line: 1, text: "Hello wor".into() }]);
    assert_eq!(parse_note("Hi\n```u", true), [Block::Prose { line: 1, text: "Hi\n".into() }]);
}

#[test]
fn a_streamed_line_keeps_its_hash_as_lines_arrive() {
    let a = parse_outline("row\n  box Today\n");
    let b = parse_outline("row\n  box Today\n  arrow\n");
    assert_eq!(a[0].kids[0].hash, b[0].kids[0].hash);
    assert_eq!(a[0].kids[0].key(), b[0].kids[0].key());
}

#[test]
fn other_fences_pass_through() {
    let b = parse_note("~~~mermaid\ngraph TD\n~~~\nafter\n", false);
    assert_eq!(b[0], Block::Fence { line: 1, lang: "mermaid".into(), body: "graph TD\n".into(), open: false });
    assert_eq!(b[1], Block::Prose { line: 4, text: "after\n".into() });
}

#[test]
fn plain_and_html_keep_every_word() {
    let n = parse_outline("box Status\n  kv Owner: t1108\n  say Kit {voice}: ship <it>");
    assert_eq!(to_plain(&n), "Status\n  Owner: t1108\n  Kit: ship <it>\n");
    let h = to_html(&n);
    assert!(h.contains("<div class=\"rp-say\" data-tone=\"quiet\" data-word=\"voice\"><b>Kit</b> <span>ship &lt;it&gt;</span></div>"), "{h}");
}

#[test]
fn docs_list_every_part_or_explain_one() {
    let all = docs(None).unwrap();
    assert!(CATALOG.iter().all(|s| all.contains(s.syntax)));
    assert!(docs(Some("say")).unwrap().contains(spec("say").unwrap().example));
    assert!(docs(Some("nope")).unwrap_err().contains("no part `nope`"));
}

#[test]
fn tones_read_their_word() {
    assert_eq!(Tone::of("done"), Tone::Done);
    assert_eq!(Tone::of("Risk"), Tone::Risk);
    assert_eq!(Tone::of("proposal"), Tone::Proposal);
    assert_eq!(Tone::of("primary"), Tone::Accent);
    assert_eq!(Tone::of("voice"), Tone::Quiet);
    assert_eq!(Tone::of("none"), Tone::Quiet);
    assert_eq!(Tone::of("on"), Tone::Done);
    assert_eq!(Tone::of("one"), Tone::Quiet);
}
