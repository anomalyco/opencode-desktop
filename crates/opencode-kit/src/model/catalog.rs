//! Provider model catalog metadata and native attachment capabilities.

/// One selectable model from `/api/provider`.
#[derive(Clone, Debug)]
pub struct Model {
    pub key: String,
    pub id: String,
    pub provider: String,
    /// What the model's group header says: its provider's name from `/api/provider`, or the
    /// provider's id when the server names it nothing better.
    pub group: String,
    pub name: String,
    pub variants: Vec<String>,
    /// Left out of the list unless Manage models shows it (Desktop's default `visible`).
    pub hidden: bool,
    /// OpenCode's free models (Desktop's `isFreeModel`).
    pub free: bool,
    /// Named "(latest)" by the server: its row says Latest (`models.tsx:110`).
    pub latest: bool,
    /// What it reads, in Desktop's order (text, image, pdf, audio, video), and whether it calls
    /// tools: the flyout's summary under its efforts (`details.tsx:32-43`).
    pub inputs: Vec<String>,
    pub tools: bool,
    /// Set on a model that is another model of its provider served faster or slower
    /// ("gpt-5.5-fast" for "gpt-5.5", the server's `experimental.speed`): it is offered as a
    /// speed of that model, not as a row of its own (`packages/schema/src/model.ts` `Speed`).
    pub speed: Option<Speed>,
}

/// A model's place among another model's speeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Speed {
    /// The id of the same provider's model at its standard speed.
    pub base: String,
    /// The speed's name: "fast", "flex".
    pub id: String,
    /// Faster than the standard speed; a slower one trades latency for a lower price.
    pub faster: bool,
}

/// What a model reads natively besides text (`AttachmentDestination.input`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttachmentInput {
    pub image: bool,
    pub pdf: bool,
}
