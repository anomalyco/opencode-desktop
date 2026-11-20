//! The host functions a live program reaches: `__input`, and under `__host`: `post` (sync),
//! `receive` and `sleep` (async, idempotent, so a paused program can be saved while they wait),
//! `now` (sync) and `fetch` (async). Each async call is a future of its own here, so the run's
//! rests are seen when its futures are polled again ([`super::link::Watch`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use psychoscript::{Call, HostError, HostFunction, HostResult, HostValue, Runtime};
use serde_json::{Value, json};

use super::fetch;
use super::link::{Link, Run};
use super::Post;
use crate::draw::DrawOutput;
use crate::node::Node;
use crate::step::SendMessage;

fn throw(message: String) -> HostValue {
    HostValue::Error(Box::new(HostError::new(message)))
}

/// The runtime of run `era` of `link`'s program, its globals bound to `link`.
pub(super) fn runtime(link: &Arc<Link>, era: u64) -> Runtime {
    let link = link.clone();
    Runtime::make().globals(move |_run| {
        let input = json!({ "data": link.data });
        let (l1, l2, l3, l4, l5) = (link.clone(), link.clone(), link.clone(), link.clone(), link.clone());
        let host = HostValue::object([
            ("post", HostValue::Function(HostFunction::new(move |args: Vec<HostValue>, _call: &Call| post(&l1, args)))),
            ("now", HostValue::Function(HostFunction::new(move |_args: Vec<HostValue>, _call: &Call| -> HostResult { Ok(HostValue::Number(l2.now_ms())) }))),
            ("receive", HostValue::Function(HostFunction::new_async(move |_args, _call| Receive { link: l3.clone(), era, ticket: None, polls: 0 }).idempotent())),
            (
                "sleep",
                HostValue::Function(
                    HostFunction::new_async(move |args: Vec<HostValue>, call: Call| {
                        let ms = args.first().and_then(HostValue::as_f64).filter(|m| m.is_finite()).unwrap_or(0.0).max(0.0);
                        Sleep { link: l4.clone(), era, key: call.key(), ms, polls: 0 }
                    })
                    .idempotent(),
                ),
            ),
            (
                "fetch",
                HostValue::Function(HostFunction::new_async(move |args: Vec<HostValue>, _call: Call| Fetch {
                    link: l5.clone(),
                    era,
                    request: Some(args.first().and_then(HostValue::to_json).unwrap_or(Value::Null)),
                    id: 0,
                    polls: 0,
                })),
            ),
        ]);
        [("__host", host), ("__input", HostValue::Function(HostFunction::new(move |_args: Vec<HostValue>, _call: &Call| -> HostResult { Ok(HostValue::from_json(&input)) })))]
    })
}

/// `__host.post(message)`: checked and handed to the client; a bad view or draw throws.
fn post(link: &Link, args: Vec<HostValue>) -> HostResult {
    if link.lock().interrupt {
        return Err(HostValue::Interrupt);
    }
    let message = args.first().and_then(HostValue::to_json).unwrap_or(Value::Null);
    let post = match message["type"].as_str() {
        Some("render") => Post::Render(Node::from_value(&message["node"]).map_err(|e| throw(format!("widget.render: {e}")))?),
        Some("draw") => {
            let id = message["id"].as_str().ok_or_else(|| throw("widget.draw needs a canvas id".into()))?.to_owned();
            let out = DrawOutput::from_value(&json!({ "commands": message["commands"] })).map_err(|e| throw(format!("widget.draw(\"{id}\"): {e}")))?;
            Post::Draw { id, commands: out.commands }
        }
        Some("frame") => Post::Frame,
        Some("submit") => Post::Submit(message["value"].clone()),
        Some("dismiss") => Post::Dismiss,
        Some("capture") => Post::Capture(message["value"].as_bool().unwrap_or(false)),
        Some("send") => Post::Send(SendMessage { value: message["value"].clone(), wake: message["wake"].as_bool().unwrap_or(true) }),
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
            st.held.push(cx.waker().clone());
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
            st.held.push(cx.waker().clone());
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
            st.held.push(cx.waker().clone());
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
    let request = fetch::request(&request, grants)?;
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
