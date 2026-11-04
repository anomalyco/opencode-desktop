use super::*;

#[test]
fn interrupt_ack_preserves_idle_running_and_legacy_responses() {
    for (body, expected) in [(r#"{"interrupted":false}"#, Some(false)), (r#"{"interrupted":true}"#, Some(true)), ("{}", None)] {
        let (base, request) = mock_replies(vec![("200 OK", body.into())]);
        assert_eq!(Server::new(&base).interrupt_ack("ses_test").unwrap(), expected);
        assert!(request.join().unwrap()[0].starts_with("POST /api/session/ses_test/interrupt HTTP/1.1"));
    }
}
