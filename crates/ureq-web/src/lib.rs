//! The part of ureq's API the app and its crates use, over a page's `fetch` or synchronous
//! `XMLHttpRequest`: the same request code runs natively and in a browser. A file that makes
//! requests names it `ureq` on the web (`#[cfg(target_arch = "wasm32")] use ureq_web as ureq;`).
//! The app makes its requests from `rune::task` work, where they're a `fetch` the work waits
//! on without holding the page.
//!
//! What a browser can't do: a page's synchronous request has no time limit (the timeouts are
//! taken and ignored), and the whole body arrives before the call returns.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::fmt;
use std::io::{Cursor, Read};
use std::marker::PhantomData;
use std::time::Duration;

pub use http;
use rune::time::Instant;
use wasm_bindgen::{JsCast, JsValue};

/// A request's failure, worded as ureq words it, so callers' checks on the text read the same.
#[derive(Debug)]
pub enum Error {
    /// The server answered with this status (only when statuses count as errors).
    StatusCode(u16),
    /// No answer in time. A browser reports none: its synchronous requests can't time out.
    Timeout(Timeout),
    /// The browser couldn't make the request: no server, or CORS refused the page's origin.
    Io(String),
}

#[derive(Debug, Clone, Copy)]
pub struct Timeout;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::StatusCode(n) => write!(f, "http status: {n}"),
            Error::Timeout(_) => write!(f, "timeout: global"),
            Error::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Default)]
pub struct Agent {
    status_errors: bool,
}

pub struct Config {
    status_errors: bool,
}

impl Agent {
    pub fn config_builder() -> Config {
        Config { status_errors: true }
    }
    pub fn get(&self, url: impl AsRef<str>) -> RequestBuilder<WithoutBody> {
        self.request("GET", url)
    }
    pub fn delete(&self, url: impl AsRef<str>) -> RequestBuilder<WithoutBody> {
        self.request("DELETE", url)
    }
    pub fn post(&self, url: impl AsRef<str>) -> RequestBuilder<WithBody> {
        self.request("POST", url)
    }
    pub fn put(&self, url: impl AsRef<str>) -> RequestBuilder<WithBody> {
        self.request("PUT", url)
    }
    pub fn patch(&self, url: impl AsRef<str>) -> RequestBuilder<WithBody> {
        self.request("PATCH", url)
    }
    /// Sends a request built with `http`'s builder, as `ureq::Agent::run` does.
    pub fn run<B: AsSendBody>(&self, request: http::Request<B>) -> Result<Response, Error> {
        let (parts, body) = request.into_parts();
        let bytes = body.into_bytes().map_err(|e| Error::Io(e.to_string()))?;
        let headers: Vec<(&str, &str)> = parts.headers.iter().filter_map(|(k, v)| Some((k.as_str(), v.to_str().ok()?))).collect();
        call(parts.method.as_str(), &parts.uri.to_string(), &headers, (!bytes.is_empty()).then_some(bytes.as_slice()), self.status_errors)
    }

    fn request<B>(&self, method: &'static str, url: impl AsRef<str>) -> RequestBuilder<B> {
        RequestBuilder { method, url: url.as_ref().to_owned(), headers: Vec::new(), status_errors: self.status_errors, body: PhantomData }
    }
}

impl Config {
    pub fn timeout_global(self, _: Option<Duration>) -> Self {
        self
    }
    pub fn timeout_connect(self, _: Option<Duration>) -> Self {
        self
    }
    pub fn timeout_per_read(self, _: Option<Duration>) -> Self {
        self
    }
    pub fn http_status_as_error(mut self, on: bool) -> Self {
        self.status_errors = on;
        self
    }
    /// A page's requests follow redirects whatever they ask (a cross-origin one fails CORS).
    pub fn max_redirects(self, _: u32) -> Self {
        self
    }
    /// The browser chooses the page's proxy.
    pub fn proxy(self, _: Option<Proxy>) -> Self {
        self
    }
    /// The browser keeps its own connections.
    pub fn max_idle_age(self, _: Duration) -> Self {
        self
    }
    pub fn build(self) -> Self {
        self
    }
}

/// ureq's proxy setting, which a page has no say in.
pub struct Proxy;

/// A request's own settings (`RequestBuilder::config`): only its time limit, which a browser's
/// synchronous request can't keep anyway.
pub struct RequestConfig<B>(RequestBuilder<B>);

impl<B> RequestConfig<B> {
    pub fn timeout_global(self, _: Option<Duration>) -> Self {
        self
    }
    pub fn build(self) -> RequestBuilder<B> {
        self.0
    }
}

impl From<Config> for Agent {
    fn from(config: Config) -> Agent {
        Agent { status_errors: config.status_errors }
    }
}

/// `ureq::get`: a GET on a default agent.
pub fn get(url: impl AsRef<str>) -> RequestBuilder<WithoutBody> {
    Agent::from(Agent::config_builder()).get(url)
}

fn call(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>, status_errors: bool) -> Result<Response, Error> {
    let (status, list, bytes) = request_with_headers(method, url, headers, body).map_err(Error::Io)?;
    if status_errors && status >= 400 {
        return Err(Error::StatusCode(status));
    }
    let mut headers = http::HeaderMap::new();
    for (k, v) in list {
        if let (Ok(k), Ok(v)) = (http::HeaderName::from_bytes(k.as_bytes()), http::HeaderValue::from_str(&v)) {
            headers.append(k, v);
        }
    }
    Ok(Response { status: http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::OK), headers, body: Body(bytes) })
}

pub struct WithBody;
pub struct WithoutBody;

pub struct RequestBuilder<B> {
    method: &'static str,
    url: String,
    headers: Vec<(String, String)>,
    status_errors: bool,
    body: PhantomData<B>,
}

impl<B> RequestBuilder<B> {
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        self.headers.push((name.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    pub fn config(self) -> RequestConfig<B> {
        RequestConfig(self)
    }

    fn run(self, body: Option<&[u8]>) -> Result<Response, Error> {
        let headers: Vec<(&str, &str)> = self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        call(self.method, &self.url, &headers, body, self.status_errors)
    }
}

impl RequestBuilder<WithoutBody> {
    pub fn call(self) -> Result<Response, Error> {
        self.run(None)
    }
    pub fn force_send_body(self) -> RequestBuilder<WithBody> {
        RequestBuilder { method: self.method, url: self.url, headers: self.headers, status_errors: self.status_errors, body: PhantomData }
    }
}

impl RequestBuilder<WithBody> {
    pub fn send(self, body: impl AsSendBody) -> Result<Response, Error> {
        let bytes = body.into_bytes().map_err(|e| Error::Io(e.to_string()))?;
        self.run(Some(&bytes))
    }
}

/// What `send` takes: bytes, or a reader read to its end first.
pub trait AsSendBody {
    fn into_bytes(self) -> std::io::Result<Vec<u8>>;
}

impl AsSendBody for () {
    fn into_bytes(self) -> std::io::Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

impl AsSendBody for String {
    fn into_bytes(self) -> std::io::Result<Vec<u8>> {
        Ok(self.into_bytes())
    }
}

impl AsSendBody for Vec<u8> {
    fn into_bytes(self) -> std::io::Result<Vec<u8>> {
        Ok(self)
    }
}

impl AsSendBody for &str {
    fn into_bytes(self) -> std::io::Result<Vec<u8>> {
        Ok(self.as_bytes().to_vec())
    }
}

impl AsSendBody for &[u8] {
    fn into_bytes(self) -> std::io::Result<Vec<u8>> {
        Ok(self.to_vec())
    }
}

impl<const N: usize> AsSendBody for &[u8; N] {
    fn into_bytes(self) -> std::io::Result<Vec<u8>> {
        Ok(self.to_vec())
    }
}

impl AsSendBody for &Vec<u8> {
    fn into_bytes(self) -> std::io::Result<Vec<u8>> {
        Ok(self.clone())
    }
}

impl AsSendBody for SendBody<'_> {
    fn into_bytes(self) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.0.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

pub struct SendBody<'a>(&'a mut dyn Read);

impl<'a> SendBody<'a> {
    pub fn from_reader(reader: &'a mut dyn Read) -> SendBody<'a> {
        SendBody(reader)
    }
}

pub struct Response {
    status: http::StatusCode,
    headers: http::HeaderMap,
    body: Body,
}

impl Response {
    pub fn status(&self) -> http::StatusCode {
        self.status
    }
    pub fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }
    pub fn body_mut(&mut self) -> &mut Body {
        &mut self.body
    }
    pub fn into_body(self) -> Body {
        self.body
    }
}

pub struct Body(Vec<u8>);

impl Body {
    pub fn read_to_string(&mut self) -> Result<String, Error> {
        String::from_utf8(std::mem::take(&mut self.0)).map_err(|e| Error::Io(e.to_string()))
    }
    pub fn read_to_vec(&mut self) -> Result<Vec<u8>, Error> {
        Ok(std::mem::take(&mut self.0))
    }
    pub fn with_config(&mut self) -> BodyConfig<'_> {
        BodyConfig { body: self, limit: u64::MAX }
    }
    pub fn into_reader(self) -> impl Read {
        Cursor::new(self.0)
    }
    pub fn as_reader(&mut self) -> impl Read + '_ {
        Cursor::new(std::mem::take(&mut self.0))
    }
}

pub struct BodyConfig<'a> {
    body: &'a mut Body,
    limit: u64,
}

impl BodyConfig<'_> {
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = limit;
        self
    }
    pub fn read_to_string(self) -> Result<String, Error> {
        self.check()?;
        self.body.read_to_string()
    }
    pub fn read_to_vec(self) -> Result<Vec<u8>, Error> {
        self.check()?;
        self.body.read_to_vec()
    }
    fn check(&self) -> Result<(), Error> {
        if self.body.0.len() as u64 > self.limit {
            return Err(Error::Io(format!("body larger than {} bytes", self.limit)));
        }
        Ok(())
    }
}

/// A request: the status the server answered and its body, or why the browser couldn't ask (no
/// server there, or its CORS rules refuse this page's origin).
pub fn request_status(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> Result<(u16, Vec<u8>), String> {
    request_with_headers(method, url, headers, body).map(|(status, _, bytes)| (status, bytes))
}

/// [`request_status`] with the response's headers, lowercased as the browser reports them.
///
/// From task work (`rune::task`, on its fibers) it's a `fetch` the work waits on while the page
/// goes on drawing and taking input. Anywhere else it's a synchronous `XMLHttpRequest`, which
/// holds the page until the answer comes: the app makes its requests from task work.
pub fn request_with_headers(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
    let failed = |_| "connection failed: the browser couldn't reach the server".to_owned();
    if let Some(answer) = fetch(method, url, headers, body) {
        return answer.map_err(failed);
    }
    let xhr = send(method, url, headers, body).map_err(failed)?;
    let status = xhr.status().map_err(failed)?;
    if status == 0 {
        return Err(failed(JsValue::NULL));
    }
    let raw = xhr.get_all_response_headers().unwrap_or_default();
    let list = raw.lines().filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    Ok((status, list, body_bytes(&xhr)))
}

/// A GET [`start_get`] began, for the next GET of `url` with `auth` from task work to take up.
struct Started {
    url: String,
    /// Its `Authorization`, empty for none.
    auth: String,
    at: Instant,
    promise: js_sys::Promise,
}

thread_local! {
    static STARTED: RefCell<Vec<Started>> = const { RefCell::new(Vec::new()) };
}

/// How long a started GET waits to be taken up before it's dropped, so a later read never gets
/// an old answer.
const STARTED_KEEP: Duration = Duration::from_secs(10);

/// Starts a GET of `url` now, with the headers the request that wants it will send: the browser
/// makes it while the page does other work (loads, draws its first frame, or waits on another
/// request), and the next GET of `url` from task work waits on this one instead of asking again.
/// Several started at once are answered at once, where task work asks one after another. Not
/// again while one for `url` is waiting to be taken up.
///
/// The page may have asked before the module loaded (`window.opencodeStarted`, a `Map` of URL to
/// `{ auth, promise }`): the first call takes all of those over, from then on as if started here.
pub fn start_get(url: &str, headers: &[(&str, &str)]) {
    let Some(window) = web_sys::window() else { return };
    let auth = authorization(headers).to_owned();
    STARTED.with_borrow_mut(|started| {
        started.extend(page_started(&window));
        started.retain(|s| s.at.elapsed() < STARTED_KEEP);
        if started.iter().any(|s| s.url == url && s.auth == auth) {
            return;
        }
        if let Ok(request) = build("GET", url, headers, None) {
            started.push(Started { url: url.to_owned(), auth, at: Instant::now(), promise: window.fetch_with_request(&request) });
        }
    });
}

/// What the page started before the module loaded, taken out of the page.
fn page_started(window: &web_sys::Window) -> Vec<Started> {
    let name = JsValue::from_str("opencodeStarted");
    let Some(map) = js_sys::Reflect::get(window, &name).ok().and_then(|m| m.dyn_into::<js_sys::Map>().ok()) else { return Vec::new() };
    let _ = js_sys::Reflect::delete_property(window, &name);
    let field = |entry: &JsValue, name: &str| js_sys::Reflect::get(entry, &JsValue::from_str(name)).ok();
    let mut out = Vec::new();
    map.for_each(&mut |entry, url| {
        let (Some(url), Some(auth), Some(promise)) = (url.as_string(), field(&entry, "auth").and_then(|a| a.as_string()), field(&entry, "promise").and_then(|p| p.dyn_into::<js_sys::Promise>().ok())) else { return };
        out.push(Started { url, auth, at: Instant::now(), promise });
    });
    out
}

fn authorization<'a>(headers: &[(&str, &'a str)]) -> &'a str {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization")).map_or("", |(_, v)| *v)
}

/// Drops the GETs [`start_get`] began that nothing took up: what they'd answer is no longer
/// wanted (the first of them failed, say).
pub fn forget_started() {
    STARTED.with_borrow_mut(Vec::clear);
}

fn take_started(url: &str, headers: &[(&str, &str)]) -> Option<js_sys::Promise> {
    let auth = authorization(headers);
    STARTED.with_borrow_mut(|started| {
        started.retain(|s| s.at.elapsed() < STARTED_KEEP);
        let i = started.iter().position(|s| s.url == url && s.auth == auth)?;
        Some(started.swap_remove(i).promise)
    })
}

fn build(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> Result<web_sys::Request, JsValue> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    let h = web_sys::Headers::new()?;
    for (name, value) in headers {
        h.append(name, value)?;
    }
    init.set_headers(&h);
    if let Some(bytes) = body {
        init.set_body(&js_sys::Uint8Array::from(bytes));
    }
    web_sys::Request::new_with_str_and_init(url, &init)
}

/// The request as a `fetch` the task work waits on; `None` outside task work.
fn fetch(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> Option<Result<(u16, Vec<(String, String)>, Vec<u8>), JsValue>> {
    let window = web_sys::window()?;
    let started = if method == "GET" && body.is_none() { take_started(url, headers) } else { None };
    let promise = match started {
        Some(promise) => promise,
        None => match build(method, url, headers, body) {
            Ok(request) => window.fetch_with_request(&request),
            Err(e) => return Some(Err(e)),
        },
    };
    let response = match rune::task::wait(&promise)? {
        Ok(r) => r.unchecked_into::<web_sys::Response>(),
        Err(e) => return Some(Err(e)),
    };
    let status = response.status();
    let mut list = Vec::new();
    if let Ok(Some(entries)) = js_sys::try_iter(&response.headers()) {
        for entry in entries.flatten() {
            let pair = js_sys::Array::from(&entry);
            if let (Some(k), Some(v)) = (pair.get(0).as_string(), pair.get(1).as_string()) {
                list.push((k.to_ascii_lowercase(), v));
            }
        }
    }
    let buffer = match response.array_buffer() {
        Ok(promise) => rune::task::wait(&promise)?,
        Err(e) => Err(e),
    };
    Some(buffer.map(|b| (status, list, js_sys::Uint8Array::new(&b).to_vec())))
}

fn send(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> Result<web_sys::XmlHttpRequest, JsValue> {
    let xhr = web_sys::XmlHttpRequest::new()?;
    xhr.open_with_async(method, url, false)?;
    // A synchronous request on a page can't ask for bytes; this charset hands each byte back as
    // one character.
    xhr.override_mime_type("text/plain; charset=x-user-defined")?;
    for (name, value) in headers {
        xhr.set_request_header(name, value)?;
    }
    match body {
        Some(bytes) => xhr.send_with_opt_u8_array(Some(bytes))?,
        None => xhr.send()?,
    }
    Ok(xhr)
}

fn body_bytes(xhr: &web_sys::XmlHttpRequest) -> Vec<u8> {
    let text = xhr.response_text().ok().flatten().unwrap_or_default();
    text.chars().map(|c| (c as u32 & 0xff) as u8).collect()
}