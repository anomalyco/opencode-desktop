use rune::{Env, Runtime, Tx};

use super::*;

#[test]
fn standalone_refcell_host_folds_opens_diagram_and_settles_at_zoom_1_6() {
    let mut host = Host { zoom: 1.6, ..Host::default() };
    host.reload(false);
    let mut rt = Runtime::new(host, view, (1280.0, 820.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    for _ in 0..60 {
        if !rt.frame(1.0 / 120.0) {
            break;
        }
    }
    assert_eq!(rt.scroll_at_bottom(&Host::scroll_key()), Some(true));
    let texts = |rt: &Runtime<Host>| -> Vec<String> {
        rt.elements().into_iter().filter_map(|e| e.text).collect()
    };
    assert!(texts(&rt).iter().any(|t| t.contains("Read more")));

    rt.update(toggle_fold);
    rt.frame(1.0 / 120.0);
    assert!(texts(&rt).iter().any(|t| t.contains("Show less")));

    rt.update(|s, tx| {
        crate::diagram::open_source(s, DIAGRAM_SRC, tx);
    });
    rt.frame(1.0 / 120.0);
    assert!(rt.state.view.diagram.is_open());
    rt.update(crate::diagram::close);
    rt.frame(1.0 / 120.0);
    for _ in 0..60 {
        if !rt.frame(1.0 / 120.0) {
            break;
        }
    }
    assert!(!rt.state.view.diagram.is_open());
    assert!(!poll(&mut rt.state, 5.0, &mut Tx::default()));
}
