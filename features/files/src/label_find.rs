//! Re-export of `opencode_code_view::data::label_find` plus the Mermaid diagram find alias.

use std::rc::Rc;

#[allow(unused_imports)]
pub(crate) use opencode_code_view::data::label_find::{Document, LabelDoc, LabelFind, TableFind};

pub(crate) type DiagramFind = LabelFind<Rc<super::mermaid::Diagram>>;
