//! A reply that streams a flowchart and a sequence diagram, for the fixture and the storybook.
//!
//! `--scene diagram-stream` streams it into the fixture session. The story (Timeline › Diagram
//! streaming) streams it again on R; F finishes it. Each diagram draws from its complete lines:
//! nodes fade and grow in, edges and messages draw on from source to target, and the fence never
//! shows as code.

use opencode_kit::model::Message;
use rune::Play;

use super::demo::{finish, stream};
use crate::App;
use crate::stories::{Trigger, replay, sid};

pub const ASK: &str = "How does a reconnect work now, end to end?";

/// Prose, a flowchart and a sequence diagram.
pub const REPLY: &str = r#"A dropped socket retries at once, then backs off until it's back or two tries have failed:

```mermaid
flowchart LR
  Drop[Socket drops] --> R1[Retry at once]
  R1 -->|ok| On[Online]
  R1 -->|fails| R2[Back off 1 s]
  R2 -->|ok| On
  R2 -->|fails| Off[Offline]
  Off -->|retry| R2
```

And who talks to whom once it's back:

```mermaid
sequenceDiagram
  participant App
  participant Server
  participant Inbox
  App->>Server: Reconnect
  Server-->>App: Resume from cursor
  App->>Inbox: Replay queued prompts
  Inbox-->>App: Sent in order
```

Nothing changes on the server; the queue keeps its order.
"#;

const ASK_ID: &str = "msg_zzdiagram0";
const REPLY_ID: &str = "msg_zzdiagram1";

fn prompt(s: &mut App) {
    let id = sid(s);
    if let Some(t) = s.transcripts.get_mut(&id) {
        t.messages.push(Message::User { id: ASK_ID.into(), text: ASK.into() });
    }
}

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    prompt(s);
    stream(s, REPLY_ID, REPLY, 0.08);
}

pub const KEYS: &[Trigger] = &[replay(open), Trigger { key: 'f', label: "Finish the stream", run: finish }];

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Diagram streaming", KEYS).knobs(crate::stories::ROW_KNOBS),
]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("diagram-stream", |s| {
        crate::open_scene(s);
        s.finish_stream();
        prompt(s);
        stream(s, REPLY_ID, REPLY, 0.08);
    }),
]);
