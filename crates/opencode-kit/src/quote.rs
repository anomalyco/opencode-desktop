//! Text quoted from the conversation into a prompt, as Desktop sends it: no part of its own,
//! just lines in the prompt's `text` and a `{type: "quote"}` entry in `metadata.comments`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

/// One quote (Desktop's `QuoteComment`), with the comment the user wrote about it, if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quote {
    pub text: String,
    pub comment: Option<String>,
}

impl Quote {
    /// What the model reads (Desktop's `formatQuoteComment`). Every quoted line is prefixed,
    /// so quoted text can't end the quote early and read as the user's own words.
    pub fn prompt_text(&self) -> String {
        let lines: Vec<String> = self.text.split('\n').map(|l| format!("> {l}")).collect();
        let quote = format!("The user quoted the following from earlier in the conversation:\n{}", lines.join("\n"));
        match self.comment() {
            Some(c) => format!("{quote}\n\nThe user made the following comment regarding this quote: {c}"),
            None => quote,
        }
    }

    /// The trimmed comment, when there's one to send.
    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref().map(str::trim).filter(|c| !c.is_empty())
    }

    /// Its `metadata.comments` entry: the comment travels only when the user wrote one.
    pub fn json(&self) -> Value {
        let mut v = serde_json::json!({ "type": "quote", "text": self.text });
        if let Some(c) = self.comment() {
            v["comment"] = c.into();
        }
        v
    }

    pub fn parse(v: &Value) -> Option<Quote> {
        if v["type"] != "quote" {
            return None;
        }
        let text = v["text"].as_str()?.to_owned();
        Some(Quote { text, comment: v["comment"].as_str().map(str::to_owned) })
    }
}

/// The quotes in a user message's `metadata`, read only beside a `displayText`, as Desktop's
/// `readPromptPresentation` does (prompts from other clients have no comments list).
pub fn parse_metadata(metadata: &Value) -> Option<(String, Vec<Quote>)> {
    let display = metadata["displayText"].as_str()?;
    let comments = metadata["comments"].as_array()?;
    Some((display.to_owned(), comments.iter().filter_map(Quote::parse).collect()))
}

/// User messages' quotes, by message id, kept beside `Message::User` like its files.
fn store() -> &'static Mutex<HashMap<String, Arc<[Quote]>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Arc<[Quote]>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// The quotes on user message `id` (empty when it has none).
pub fn user_quotes(id: &str) -> Arc<[Quote]> {
    store().lock().unwrap().get(id).cloned().unwrap_or_else(|| Arc::from([]))
}

/// Records the quotes on user message `id`, as a local echo of a sent prompt has them.
pub fn set_user_quotes(id: &str, quotes: Vec<Quote>) {
    let mut store = store().lock().unwrap();
    if quotes.is_empty() {
        store.remove(id);
    } else {
        store.insert(id.to_owned(), quotes.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_text_prefixes_every_line() {
        let q = Quote { text: "one\ntwo".into(), comment: None };
        assert_eq!(q.prompt_text(), "The user quoted the following from earlier in the conversation:\n> one\n> two");
        let q = Quote { text: "x".into(), comment: Some("  why?  ".into()) };
        assert_eq!(
            q.prompt_text(),
            "The user quoted the following from earlier in the conversation:\n> x\n\nThe user made the following comment regarding this quote: why?"
        );
        assert_eq!(Quote { text: "x".into(), comment: Some(" ".into()) }.json(), serde_json::json!({"type": "quote", "text": "x"}));
    }

    #[test]
    fn metadata_needs_display_text_and_comments() {
        let m = serde_json::json!({"displayText": "hi", "comments": [{"type": "quote", "text": "a"}, {"type": "note", "comment": "n"}]});
        assert_eq!(parse_metadata(&m), Some(("hi".into(), vec![Quote { text: "a".into(), comment: None }])));
        assert_eq!(parse_metadata(&serde_json::json!({"comments": []})), None);
    }
}
