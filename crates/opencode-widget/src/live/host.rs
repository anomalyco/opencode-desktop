//! The host functions a live program reaches: `__input`, and under `__host`: `post` (sync),
//! `receive` and `sleep` (async, idempotent, so a paused program can be saved while they wait),
//! `now` (sync) and `fetch` (async). Each async call is a future of its own here, so the run's
//! rests are seen when its futures are polled again ([`super::link::Watch`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use psychoscript::{Call, HostError, HostFunction, HostResult, HostValue, Limits, Runtime, Translate, TranslateFunctions};
use serde_json::{Value, json};

use super::fetch;
use super::link::{Link, Run};
use super::Post;
use crate::draw::{Arg, DrawOutput};
use crate::node::Node;
use crate::step::SendMessage;

fn throw(message: String) -> HostValue {
    HostValue::Error(Box::new(HostError::new(message)))
}

/// The runtime of run `era` of `link`'s program, its globals bound to `link`, its heap capped at
/// [`crate::runner::MAX_HEAP_BYTES`] (a saved run keeps the cap).
pub(super) fn runtime(link: &Arc<Link>, era: u64) -> Runtime {
    let link = link.clone();
    // A function in a post's view (a handler) crosses as a marker; anywhere else it fails the call.
    let translate = Translate { functions: TranslateFunctions::Mark, ..Default::default() };
    Runtime::make().limits(Limits { max_heap_bytes: Some(crate::runner::MAX_HEAP_BYTES), ..Default::default() }).translate(translate).globals(move |_run| {
        let input = json!({ "data": link.data });
        let (l1, l2, l3, l4, l5) = (link.clone(), link.clone(), link.clone(), link.clone(), link.clone());
        let host = HostValue::object([
            ("post", HostValue::Function(HostFunction::new(move |args: Vec<HostValue>, _call: &Call| post(&l1, args)))),
            (
                "now",
                HostValue::Function(HostFunction::new(move |args: Vec<HostValue>, _call: &Call| -> HostResult {
                    checked("now", &args, 0)?;
                    Ok(HostValue::Number(l2.now_ms()))
                })),
            ),
            (
                "receive",
                HostValue::Function(
                    HostFunction::new_async(move |args: Vec<HostValue>, _call| Checked::new(checked("receive", &args, 0), || Receive { link: l3.clone(), era, ticket: None, polls: 0 })).idempotent(),
                ),
            ),
            (
                "sleep",
                HostValue::Function(
                    HostFunction::new_async(move |args: Vec<HostValue>, call: Call| {
                        let ms = args.first().and_then(HostValue::as_f64).filter(|m| m.is_finite()).unwrap_or(0.0).max(0.0);
                        Checked::new(checked("sleep", &args, 0), || Sleep { link: l4.clone(), era, key: call.key(), ms, polls: 0 })
                    })
                    .idempotent(),
                ),
            ),
            (
                "fetch",
                HostValue::Function(HostFunction::new_async(move |args: Vec<HostValue>, _call: Call| {
                    Checked::new(checked("fetch", &args, 0), || Fetch {
                        link: l5.clone(),
                        era,
                        request: Some(args.first().and_then(HostValue::to_json).unwrap_or(Value::Null)),
                        id: 0,
                        polls: 0,
                    })
                })),
            ),
        ]);
        [("__host", host), ("__input", HostValue::Function(HostFunction::new(move |_args: Vec<HostValue>, _call: &Call| -> HostResult { Ok(HostValue::from_json(&input)) })))]
    })
}

/// What `post` throws for a function in its arguments.
const POSTED: &str = "Argument 1 to __host.post";

/// Fails with the runtime's `TypeError` when an argument of `__host.<name>`, from `from` on, holds a
/// function (`widgets.md`, Native hosts: functions where none may go).
fn checked(name: &str, args: &[HostValue], from: usize) -> Result<(), HostValue> {
    for (i, arg) in args.iter().enumerate().skip(from) {
        crate::tree::strict(arg, &format!("Argument {} to __host.{name}", i + 1))?;
    }
    Ok(())
}

/// An async host call's future, or what its arguments throw before it starts.
enum Checked<F> {
    Thrown(Option<HostValue>),
    Go(F),
}

impl<F> Checked<F> {
    fn new(args: Result<(), HostValue>, go: impl FnOnce() -> F) -> Self {
        match args {
            Ok(()) => Checked::Go(go()),
            Err(e) => Checked::Thrown(Some(e)),
        }
    }
}

impl<F: Future<Output = HostResult> + Unpin> Future for Checked<F> {
    type Output = HostResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<HostResult> {
        match self.get_mut() {
            Checked::Thrown(e) => Poll::Ready(Err(e.take().unwrap_or(HostValue::Undefined))),
            Checked::Go(f) => Pin::new(f).poll(cx),
        }
    }
}

/// `__host.post(message)`: checked and handed to the client (`widgets.md`, Native hosts). A
/// render's view is made into its tree here, and `post` says whether it has handlers; a view that
/// breaks Keys and depth ends the run at once (phase `view`). A draw's key names a canvas of the
/// last render. A function anywhere but a view's handler props, or a bad view or draw, throws.
fn post(link: &Link, mut args: Vec<HostValue>) -> HostResult {
    if link.lock().interrupt {
        return Err(HostValue::Interrupt);
    }
    // Read from the VM's value as from its JSON, without making it (`host_value`).
    let mut message = if args.is_empty() { HostValue::Null } else { std::mem::take(&mut args[0]) };
    if message.field("type").and_then(Arg::text).as_deref() == Some("render") {
        let mut take = |name: &str| match &mut message {
            HostValue::Object(fields) => fields.iter_mut().find(|(k, _)| k.as_str() == Some(name)).map(|(_, v)| std::mem::take(v)).unwrap_or_default(),
            _ => HostValue::Undefined,
        };
        let (node, failure) = (take("node"), take("failure"));
        let built = match failure {
            HostValue::String(f) => Err(f.to_string_lossy().into_owned()),
            _ => match crate::tree::build(node, POSTED) {
                Ok(tree) => {
                    crate::tree::strict(&message, POSTED)?;
                    Ok(tree)
                }
                Err(crate::tree::Fail::View(m)) => Err(m),
                Err(crate::tree::Fail::Thrown(e)) => return Err(e),
            },
        };
        checked("post", &args, 1)?;
        let tree = match built {
            Ok(tree) => tree,
            Err(m) => return Err(link.fail_view(m)),
        };
        let interactive = !tree.handlers.is_empty();
        // The canvases of the last view, read or not, rendered or dropped over the rate.
        link.lock().canvases = tree.canvases;
        let node = Node::from_host(&tree.view).map_err(|e| throw(format!("widget.render: {e}")))?;
        link.post(Post::Render(node));
        return Ok(HostValue::Bool(interactive));
    }
    let message = &message;
    let field = |key: &str| message.field(key);
    let json = |key: &str| field(key).and_then(HostValue::to_json).unwrap_or(Value::Null);
    let ty = field("type").and_then(Arg::text);
    // A draw's key is looked up first: its errors come before a function in it.
    if ty.as_deref() != Some("draw") {
        crate::tree::strict(message, POSTED)?;
        checked("post", &args, 1)?;
    }
    let post = match ty.as_deref() {
        Some("draw") => {
            let key = field("key").and_then(Arg::text).unwrap_or_default();
            let id = match link.lock().canvases.get(key.as_ref()) {
                None => return Err(throw(format!("No canvas with key \"{key}\" in the rendered view"))),
                Some((_, true)) => return Err(throw(format!("Canvas \"{key}\" is drawn by its wgsl; it takes no draw commands"))),
                Some((id, false)) => id.clone(),
            };
            crate::tree::strict(message, POSTED)?;
            checked("post", &args, 1)?;
            let out = DrawOutput::from_host(field("commands")).map_err(|e| throw(format!("widget.draw(\"{id}\"): {e}")))?;
            Post::Draw { id, commands: out.commands }
        }
        Some("frame") => Post::Frame,
        Some("submit") => Post::Submit(json("value")),
        Some("dismiss") => Post::Dismiss,
        Some("capture") => Post::Capture(field("value").and_then(Arg::flag).unwrap_or(false)),
        Some("send") => Post::Send(SendMessage { value: json("value"), wake: field("wake").and_then(Arg::flag).unwrap_or(true) }),
        _ => return Ok(HostValue::Undefined),
    };
    link.post(post);
    Ok(HostValue::Undefined)
}

/// `await __host.receive()`: the next message, in call order.
struct Receive {
    link: Arc<Link>,
    era: u64,
    /// Its place among the receives waiting, once polled; none once served.
    ticket: Option<u64>,
    polls: u32,
}

impl Future for Receive {
    type Output = HostResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<HostResult> {
        let this = self.get_mut();
        this.polls += 1;
        let now = Instant::now();
        let link = this.link.clone();
        let mut st = link.lock();
        if st.interrupt || st.era != this.era {
            return Poll::Ready(Err(HostValue::Interrupt));
        }
        let again = this.polls > 1;
        if st.pause.is_some() {
            st.hold(cx.waker());
            if again {
                st.watch.repolled(now, true);
            }
            return Poll::Pending;
        }
        let ticket = match this.ticket {
            Some(t) => {
                if let Some(w) = st.waiters.iter_mut().find(|(k, _)| *k == t) {
                    w.1.clone_from(cx.waker());
                }
                t
            }
            None => {
                st.tickets += 1;
                let t = st.tickets;
                st.waiters.push_back((t, cx.waker().clone()));
                this.ticket = Some(t);
                t
            }
        };
        if st.waiters.front().is_some_and(|(k, _)| *k == ticket)
            && let Some(message) = st.inbox.pop_front()
        {
            st.waiters.pop_front();
            this.ticket = None;
            if !st.inbox.is_empty()
                && let Some((_, w)) = st.waiters.front()
            {
                w.wake_by_ref();
            }
            if again {
                st.watch.repolled(now, false);
            }
            return Poll::Ready(Ok(HostValue::from_json(&message)));
        }
        if again {
            st.watch.repolled(now, true);
        }
        Poll::Pending
    }
}

impl Drop for Receive {
    fn drop(&mut self) {
        let Some(t) = self.ticket else { return };
        let mut st = self.link.lock();
        let first = st.waiters.front().is_some_and(|(k, _)| *k == t);
        st.waiters.retain(|(k, _)| *k != t);
        if first
            && !st.inbox.is_empty()
            && let Some((_, w)) = st.waiters.front()
        {
            w.wake_by_ref();
        }
    }
}

/// `await __host.sleep(ms)`: due `ms` of running time after the call (the same due time again
/// when a saved run calls it anew).
struct Sleep {
    link: Arc<Link>,
    era: u64,
    key: String,
    ms: f64,
    polls: u32,
}

impl Future for Sleep {
    type Output = HostResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<HostResult> {
        let this = self.get_mut();
        this.polls += 1;
        let now = Instant::now();
        let link = this.link.clone();
        let mut st = link.lock();
        if st.interrupt || st.era != this.era {
            return Poll::Ready(Err(HostValue::Interrupt));
        }
        let again = this.polls > 1;
        if st.pause.is_some() {
            st.hold(cx.waker());
            if again {
                st.watch.repolled(now, true);
            }
            return Poll::Pending;
        }
        let running = st.running(now);
        let due = *st.dues.entry(this.key.clone()).or_insert(running + this.ms);
        if running >= due {
            st.dues.remove(&this.key);
            st.timers.remove(&this.key);
            if again {
                st.watch.repolled(now, false);
            }
            return Poll::Ready(Ok(HostValue::Undefined));
        }
        let fresh = st.timers.insert(this.key.clone(), cx.waker().clone()).is_none();
        if again {
            st.watch.repolled(now, true);
        }
        drop(st);
        if fresh {
            super::link::poke();
        }
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        let mut st = self.link.lock();
        st.timers.remove(&self.key);
        // A run being saved calls it again on resume, with its due time.
        if !matches!(st.run, Run::Saving) {
            st.dues.remove(&self.key);
        }
    }
}

/// `await __host.fetch(request)`: checked here, sent by the host's transport on a thread of its
/// own, its answer held while paused.
struct Fetch {
    link: Arc<Link>,
    era: u64,
    /// The request, until it goes.
    request: Option<Value>,
    id: u64,
    polls: u32,
}

impl Future for Fetch {
    type Output = HostResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<HostResult> {
        let this = self.get_mut();
        this.polls += 1;
        let now = Instant::now();
        let link = this.link.clone();
        let mut st = link.lock();
        if st.interrupt || st.era != this.era {
            return Poll::Ready(Err(HostValue::Interrupt));
        }
        let again = this.polls > 1;
        if let Some(request) = this.request.take() {
            let started = start(&link, &mut st, this, request, cx, now);
            drop(st);
            return match started {
                Ok(()) => Poll::Pending,
                Err(e) => {
                    link.post(Post::Failed { phase: "fetch", message: format!("{}: {}", e.kind.name(), e.message) });
                    Poll::Ready(Err(HostValue::from_json(&e.to_value())))
                }
            };
        }
        if st.pause.is_some() {
            st.hold(cx.waker());
            if again {
                st.watch.repolled(now, true);
            }
            return Poll::Pending;
        }
        if let Some(result) = st.fetched.remove(&this.id) {
            if again {
                st.watch.repolled(now, false);
            }
            return Poll::Ready(result);
        }
        if let Some(w) = st.fetches.get_mut(&this.id) {
            w.clone_from(cx.waker());
        }
        if again {
            st.watch.repolled(now, true);
        }
        Poll::Pending
    }
}

/// Checks `request` and sends it on a thread of its own; its answer comes to `link.fetched`.
fn start(link: &Arc<Link>, st: &mut super::link::St, this: &mut Fetch, request: Value, cx: &Context<'_>, now: Instant) -> Result<(), fetch::FetchError> {
    let grants = &link.setup.grants;
    let request = fetch::request(&request, grants, link.setup.allow_local_network)?;
    let transport = link.setup.transport.clone().ok_or_else(|| fetch::FetchError::new(fetch::FetchErrorKind::Network, "this client can't fetch"))?;
    st.rate.take(now)?;
    st.fetch_ids += 1;
    this.id = st.fetch_ids;
    st.fetches.insert(this.id, cx.waker().clone());
    let (id, grants, back) = (this.id, grants.clone(), link.clone());
    let sent = std::thread::Builder::new().name("widget-fetch".into()).spawn(move || {
        let (result, failure) = match fetch::run(&*transport, request, &grants) {
            Ok(v) => (Ok(HostValue::from_json(&v)), None),
            Err(e) => (Err(HostValue::from_json(&e.to_value())), Some(format!("{}: {}", e.kind.name(), e.message))),
        };
        back.fetched(id, result, failure);
    });
    if sent.is_err() {
        st.fetches.remove(&this.id);
        st.rate.in_flight = st.rate.in_flight.saturating_sub(1);
        return Err(fetch::FetchError::new(fetch::FetchErrorKind::Network, "couldn't start the request"));
    }
    Ok(())
}

impl Drop for Fetch {
    fn drop(&mut self) {
        if self.id == 0 {
            return;
        }
        let mut st = self.link.lock();
        st.fetches.remove(&self.id);
        st.fetched.remove(&self.id);
    }
}
