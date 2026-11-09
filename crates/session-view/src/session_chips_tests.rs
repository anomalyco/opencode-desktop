use super::*;

const ID: &str = "ses_fx999900aaaaaaaaaaaaaaaaaa";
const OTHER: &str = "ses_fx999901bbbbbbbbbbbbbbbbbb";

fn ids(src: &str) -> Vec<&str> {
    find(src).into_iter().map(|(_, id)| id).collect()
}

fn known(id: &str) -> Option<SessionRef> {
    (id == ID).then(|| SessionRef { title: "Review the [reconnect] fix".into(), parent: Some("Fix flaky reconnect".into()), ..Default::default() })
}

#[test]
fn a_bare_id_is_found_with_punctuation_around_it() {
    for src in [
        format!("{ID}"),
        format!("see {ID}."),
        format!("({ID})"),
        format!("ran in {ID}, then {OTHER}!"),
        format!("\"{ID}\": done"),
        format!("- {ID}\n"),
        format!("**{ID}**"),
    ] {
        let found = find(&src);
        assert_eq!(found.first().map(|(r, id)| (&src[r.clone()], *id)), Some((ID, ID)), "{src}");
    }
    assert_eq!(ids(&format!("ran in {ID}, then {OTHER}!")), [ID, OTHER]);
}

#[test]
fn a_lone_id_in_inline_code_replaces_the_span() {
    let src = format!("the pass (`{ID}`) is gone");
    let found = find(&src);
    assert_eq!(found.len(), 1);
    assert_eq!(&src[found[0].0.clone()], format!("`{ID}`"));
    assert_eq!(found[0].1, ID);
    assert_eq!(rewrite(&src, known).unwrap(), format!("the pass ([Review the \\[reconnect\\] fix](session:{ID})) is gone"));
}

#[test]
fn ids_in_code_blocks_links_and_longer_words_stay() {
    for src in [
        format!("```sh\nopencode export {ID}\n```\n"),
        format!("~~~\n{ID}\n~~~"),
        format!("`opencode export {ID}`"),
        format!("[{ID}](https://example.com)"),
        format!("[log](https://example.com/{ID})"),
        format!("<https://example.com/{ID}>"),
        format!("path/{ID}"),
        format!("@{ID}"),
        format!("session:{ID}"),
        format!("x{ID}"),
        format!("{ID}_2"),
        format!("{ID}/log"),
        "ses_short".to_owned(),
        format!("{ID}aaaaaaa"),
        "ses_fx99990".to_owned(),
        format!("\\{ID}"),
    ] {
        assert!(ids(&src).is_empty(), "{src}");
    }
}

#[test]
fn text_after_a_closed_fence_is_searched_again() {
    let src = format!("```\n{OTHER}\n```\nthen {ID}");
    assert_eq!(ids(&src), [ID]);
}

#[test]
fn only_known_ids_are_rewritten() {
    let src = format!("{ID} and {OTHER}");
    assert_eq!(rewrite(&src, known).unwrap(), format!("[Review the \\[reconnect\\] fix](session:{ID}) and {OTHER}"));
    assert_eq!(rewrite(&OTHER.to_string(), known), None);
    assert_eq!(rewrite("no ids", known), None);
}

#[test]
fn an_untitled_session_reads_as_its_short_id() {
    let r = rewrite(ID, |_| Some(SessionRef::default())).unwrap();
    assert_eq!(r, format!("[ses\\_fx999900](session:{ID})"));
}

#[test]
fn plain_ids_finds_bare_ids_in_a_prompt() {
    let src = format!("look at {ID} and @session:{OTHER}");
    assert_eq!(plain_ids(&src).into_iter().map(|(_, id)| id).collect::<Vec<_>>(), [ID]);
}

#[test]
fn the_tip_names_the_parent_and_what_it_is_doing() {
    let info = known(ID).unwrap();
    let busy = SessionRef { busy: true, ..info.clone() };
    let tip_busy = tip(&busy, ID, 0);
    assert!(tip_busy.contains("Review the [reconnect] fix") && tip_busy.contains("Fix flaky reconnect") && tip_busy.contains("Running"), "{tip_busy}");
    let idle = SessionRef { updated: 1_000, ..info };
    assert!(tip(&idle, ID, 1_000 + 5 * 60_000).contains("Active"));
    assert!(tip(&idle, ID, 0).ends_with(ID));
}
