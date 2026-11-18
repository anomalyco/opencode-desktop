//! Desktop re-export of `opencode_code_view::data::label_find` plus the Mermaid diagram find alias.

use std::rc::Rc;

#[allow(unused_imports)]
pub(super) use opencode_code_view::data::label_find::{Document, LabelDoc, LabelFind, TableFind};

pub(super) type DiagramFind = LabelFind<Rc<super::mermaid::Diagram>>;
