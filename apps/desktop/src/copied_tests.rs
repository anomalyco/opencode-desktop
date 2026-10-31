use crate::view::selection_tests::{session, settle};

fn ticks(rt: &rune::Runtime<crate::App>) -> usize {
    rt.elements().into_iter().filter(|e| !e.exiting && e.key.as_deref() == Some("tick")).count()
}

#[test]
fn copy_shows_a_tick_then_the_copy_icon_again() {
    let (mut rt, board) = session("");
    rt.poll = Some(crate::poll);
    let copy = rt.elements().into_iter().filter(|e| !e.exiting && e.key.as_deref() == Some("copy") && e.opacity > 0.5).last().expect("a reply's Copy");
    let (x, y) = (copy.rect[0] + copy.rect[2] / 2.0, copy.rect[1] + copy.rect[3] / 2.0);
    assert_eq!(ticks(&rt), 0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 120.0);
    assert!(board.text().is_some_and(|t| !t.is_empty()), "the reply is on the pasteboard");
    assert_eq!(ticks(&rt), 1, "only the pressed button ticks");
    for _ in 0..(1.3 * 120.0) as usize {
        rt.frame(1.0 / 120.0);
    }
    settle(&mut rt);
    assert_eq!(ticks(&rt), 0, "the tick goes after 1.2 s");
}
