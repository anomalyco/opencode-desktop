//! The app half of a reversible quit. Each retained window submits its own snapshot from its
//! turn; the terminal producer and the database then run on workers. No waiting on the UI loop.
//! Keeping the store live until the terminal disposition is what makes the same editor usable
//! on resume, even with the previous epoch's WAL sync still in flight.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use opencode_i18n::t;
use opencode_storage::Storage;
use rune::deferred_quit::{self as dq, Ticket};
use rune::{AppSpec, col, text};

mod status;
mod volatile;
pub(crate) use volatile::activity as workspace_activity;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Save { ticket: Ticket, epoch: u64 }
#[derive(Clone)]
enum State {
    Legacy,
    Running,
    Fallback,
    Windows(Save, HashSet<String>),
    Producers(Save),
    Database(Save, Storage),
    Checking(Save, Storage, volatile::Choice),
    Memory(Save, Storage, Arc<volatile::Risk>),
    Empty(Save, Storage,u64),
    Durable(Save, Storage, u64),
    Committed,
    /// A confirmed user quit that starts this app again (Tx::relaunch): never an install.
    Relaunching,
    LostWindow(Save),
    Failed(Save, Option<Storage>),
}
enum Event { Install, Begin(Save, HashSet<String>), Window(Save, String), Lost(Save,String), Store(Save, Storage), Finished(Save, Option<u64>), MemoryResult(Save,volatile::Outcome), Discard(Ticket,Arc<volatile::Risk>), Outdated(Ticket, u64), Confirmed(Ticket, dq::Then), Resume(Ticket), NoStore(Save), Retry(Save) }
enum Effect { None, Drain(Save), Barrier(Save, Storage), Check(Save,Storage), Release(Save, Storage), Receipt(Ticket, u64), Empty(Ticket,u64), Error(Ticket), Explain(Ticket), Force(Ticket) }
fn step(state: &mut State, event: Event) -> Effect {
    match (&*state, event) {
        (State::Legacy, Event::Install) => { *state=State::Running; Effect::None }
        (State::Durable(s,_,_), Event::Confirmed(k,dq::Then::Exit)) if s.ticket==k => { *state=State::Committed; Effect::None }
        (State::Durable(s,_,_), Event::Confirmed(k,dq::Then::Relaunch)) if s.ticket==k => { *state=State::Relaunching; Effect::None }
        (State::Fallback, Event::Begin(s, _)) => Effect::Empty(s.ticket,0),
        (State::Running, Event::Begin(s, ids)) if ids.is_empty() => { *state=State::Producers(s); Effect::Drain(s) }
        (State::Running, Event::Begin(s, ids)) => { *state=State::Windows(s,ids); Effect::None }
        (State::Windows(s, _), Event::Window(k,id)) if *s==k => {
            let State::Windows(_,ids)=state else { unreachable!() };
            ids.remove(&id);
            if ids.is_empty() { *state=State::Producers(k); Effect::Drain(k) } else { Effect::None }
        }
        (State::Windows(s,ids), Event::Lost(k,id)) if *s==k && ids.contains(&id) => { *state=State::LostWindow(k); Effect::Error(k.ticket) }
        (State::Producers(s), Event::Store(k,db)) if *s==k && db.memory_only() => { *state=State::Checking(k,db.clone(),volatile::Choice::Quit); Effect::Check(k,db) }
        (State::Producers(s), Event::Store(k,db)) if *s==k => { *state=State::Database(k,db.clone()); Effect::Barrier(k,db) }
        (State::Checking(s,db,_),Event::MemoryResult(k,volatile::Outcome::Empty(r))) if *s==k => { *state=State::Empty(k,db.clone(),r);Effect::Empty(k.ticket,r) }
        (State::Checking(s,db,_),Event::MemoryResult(k,volatile::Outcome::Saved(r))) if *s==k => { *state=State::Durable(k,db.clone(),r);Effect::Receipt(k.ticket,r) }
        (State::Checking(s,db,choice), Event::MemoryResult(k,volatile::Outcome::AtRisk(risk))) if *s==k => {
            let force=matches!(choice,volatile::Choice::Discard(shown) if **shown==risk) && db.try_seal_barrier(s.epoch,risk.revision);
            *state=State::Memory(k,db.clone(),Arc::new(risk));if force {Effect::Force(k.ticket)}else{Effect::Explain(k.ticket)}
        }
        (State::Memory(s,db,r),Event::Discard(k,shown)) if s.ticket==k && **r==*shown => {
            let check=Effect::Check(*s,db.clone());*state=State::Checking(*s,db.clone(),volatile::Choice::Discard(shown));check
        }
        (State::Durable(s,db,r) | State::Empty(s,db,r), Event::Outdated(k,offered)) if s.ticket==k && *r==offered && db.memory_only() => {
            let check=Effect::Check(*s,db.clone());*state=State::Checking(*s,db.clone(),volatile::Choice::Quit);check
        }
        (State::Producers(s), Event::NoStore(k)) if *s==k => { *state=State::Failed(k,None); Effect::Error(k.ticket) }
        (State::Database(s,db), Event::Finished(k,Some(r))) if *s==k => { *state=State::Durable(k,db.clone(),r); Effect::Receipt(k.ticket,r) }
        (State::Durable(s,db,r), Event::Outdated(k,offered)) if s.ticket==k && *r==offered => {
            let effect=Effect::Barrier(*s,db.clone()); *state=State::Database(*s,db.clone()); effect
        }
        (State::Database(s,db), Event::Finished(k,None)) if *s==k => { *state=State::Failed(k,Some(db.clone())); Effect::Error(k.ticket) }
        (State::Failed(s,Some(db)), Event::Retry(k)) if *s==k => { let db=db.clone(); *state=State::Database(k,db.clone()); Effect::Barrier(k,db) }
        (State::Failed(s,None), Event::Retry(k)) if *s==k => { *state=State::Producers(k); Effect::Drain(k) }
        (State::Database(s,db) | State::Checking(s,db,_) | State::Memory(s,db,_) | State::Empty(s,db,_) | State::Durable(s,db,_) | State::Failed(s,Some(db)), Event::Resume(k)) if s.ticket==k => {
            let effect=Effect::Release(*s,db.clone()); *state=State::Running; effect
        }
        (State::Windows(s,_) | State::Producers(s) | State::LostWindow(s) | State::Failed(s,None), Event::Resume(k)) if s.ticket==k => { *state=State::Running; Effect::None }
        _ => Effect::None,
    }
}
static STATE: Mutex<State> = Mutex::new(State::Legacy);
static CHANGED: std::sync::Condvar = std::sync::Condvar::new();
static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn lock() -> MutexGuard<'static,State> { STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) }
fn send(event: Event) {
    let effect={step(&mut lock(), event)};
    CHANGED.notify_all();
    match effect {
        Effect::Check(s,db)=>volatile::check(s,db), Effect::Explain(k)=>{status::refresh(k);k.failed();}, Effect::Force(k)=>k.quit_anyway(),
        Effect::None=>{}, Effect::Receipt(k,r)=>k.saved_through(r), Effect::Empty(k,r)=>k.nothing_to_save(r), Effect::Error(k)=>{
            status::refresh(k);
            k.failed();
            // One cancellable wait only while a barrier has failed. No storage polling while
            // a syscall is blocked or after resume. Never retry a successful disk barrier.
            let _ = std::thread::Builder::new().name("save-retry".into()).spawn(move || {
                let (state, timeout) = CHANGED.wait_timeout_while(lock(), std::time::Duration::from_secs(4), |s| matches!(s,State::Failed(save,_) if save.ticket==k)).unwrap_or_else(|e| e.into_inner());
                let retry=match &*state {State::Failed(save,_) if timeout.timed_out() && save.ticket==k => Some(*save),_=>None};
                drop(state);
                if let Some(save)=retry {send(Event::Retry(save))}
            });
        }
        Effect::Release(s,db)=>db.resume_barrier(s.epoch),
        Effect::Barrier(s,db)=>{status::refresh(s.ticket); db.save_barrier(s.epoch,move |ok|send(Event::Finished(s,ok)))},
        Effect::Drain(s)=>{
            status::refresh(s.ticket);
            if std::thread::Builder::new().name("opencode-save-barrier".into()).spawn(move || {
                crate::restore::undo::finish_closing();
                if crate::terminal::persist::finish_captures() && let Some(db)=crate::storage::barrier_store() {
                    send(Event::Store(s,db));
                } else { send(Event::NoStore(s)); }
            }).is_err() { send(Event::NoStore(s)); }
        }
    }
}
pub fn install() { dq::install(dq::Hooks { begin, activated, try_seal, overdue: status::overdue, resumed, confirmed, shutdown_reason: || t!("rune.storage.quit.title").to_owned() }).unwrap_or_else(|_|panic!("quit hook installed twice")); send(Event::Install); }
fn activated(args:&[String]) {
    if args.iter().any(|a| a=="--toast-activated" || a.eq_ignore_ascii_case("-Embedding")) {
        let args=args.to_vec();
        crate::plugin_host::tasks::run(move ||crate::notifications::take_activation(&args));
    }
}

/// Runs on the UI loop. Check both attempt and covered revision, without a blocking lock.
/// Only the successful compare-and-seal changes write admission; otherwise the UI returns
/// to its live Saving/Overdue state and the worker catches up under the original deadline.
fn try_seal(ticket: Ticket, revision: u64, intent:dq::Seal) -> bool {
    if let Ok(state) = STATE.try_lock() {
        match &*state {
            State::Fallback => return true,
            State::Durable(s, db, r) | State::Empty(s,db,r) if s.ticket == ticket && *r == revision => {
                match intent {
                    dq::Seal::Query if db.current_barrier(s.epoch,revision)=>{rune::marks::mark("save.query_current");return true},
                    dq::Seal::Exit if db.try_seal_barrier(s.epoch, revision)=>{rune::marks::mark("save.sealed");return true},
                    _=>{}
                }
            }
            _ => return false,
        }
    }
    rune::marks::mark("save.revision_unsealed");
    let failed=std::thread::Builder::new().name("save-catch-up".into()).spawn(move || send(Event::Outdated(ticket,revision))).is_err();
    if failed { ticket.failed(); }
    false
}
fn begin(ticket:Ticket) {
    let s=Save{ticket,epoch:EPOCH.fetch_add(1,std::sync::atomic::Ordering::Relaxed)+1};
    let windows=crate::windows::producers();
    rune::marks::mark("save.begin");
    rune::marks::mark(match crate::storage::writer_idle() {Some(true)=>"save.writer_idle", Some(false)=>"save.writer_busy", None=>"save.writer_unknown"});
    send(Event::Begin(s,windows.iter().map(|(id,_)|id.clone()).collect()));
    for (id,app) in windows { let host=app.clone(); let guard=Arc::new(WindowReply(Mutex::new(Some((s,id.clone()))))); app.update_if(move |a,_|{
        if matches!(&*lock(),State::Windows(k,ids) if *k==s && ids.contains(&id)) {
            volatile::name_drafts(a);a.prepare_quit_snapshot(); finish_window(a,host,s,id,guard);
        }
        false
    }); }
}

struct WindowReply(Mutex<Option<(Save,String)>>);
impl Drop for WindowReply {
    fn drop(&mut self) { if let Some((s,id))=self.0.get_mut().unwrap_or_else(|e|e.into_inner()).take() { send(Event::Lost(s,id)); } }
}
pub(crate) fn window_closed(id:&str) {
    let save=match &*lock() { State::Windows(s,ids) if ids.contains(id)=>Some(*s), _=>None };
    if let Some(s)=save { send(Event::Window(s,id.to_owned())); }
}
fn finish_window(a:&mut crate::App,host:rune::AppHandle<crate::App>,s:Save,id:String,guard:Arc<WindowReply>) {
    if !matches!(&*lock(),State::Windows(k,ids) if *k==s && ids.contains(&id)) { return; }
    // Success transfers the guard to completion; pending transfers it to the UI callback.
    let next=id.clone(); let reply=guard.clone();
    let again=move || {
        let again=host.clone();
        host.update_if(move |a,_| {finish_window(a,again,s,next,reply);false});
    };
    if a.quit_files_delivered(again.clone()) && a.quit_undo_delivered(again) { guard.0.lock().unwrap_or_else(|e|e.into_inner()).take(); send(Event::Window(s,id)); }
}
fn resumed(k:Ticket) { send(Event::Resume(k)); status::dismiss(); crate::updater::resumed_quit(); rune::marks::mark("save.resumed"); }
fn confirmed(k:Ticket,then:dq::Then) { send(Event::Confirmed(k,then)); status::dismiss(); rune::marks::mark("save.confirmed"); }
/// A disk receipt alone cannot authorize an updater. On Windows, the OS may impose session
/// end, even after cancelling a query. Only Rune's confirmed user-quit callback commits the
/// install intent. Builds without this registration retain their ordinary receipt policy.
pub(crate) fn permits_update() -> bool { matches!(&*lock(), State::Legacy | State::Committed) }
#[derive(Clone,PartialEq,Eq)]
enum Stage { Windows, Database, Failed, Volatile(Arc<volatile::Risk>) }
fn stage(ticket:Ticket)->Option<Stage> {
    match &*lock(){
        State::Windows(k,_)|State::Producers(k) if k.ticket==ticket =>Some(Stage::Windows),
        State::Database(k,_)|State::Durable(k,_,_)|State::Checking(k,_,_)|State::Empty(k,_,_) if k.ticket==ticket=>Some(Stage::Database),
        State::Memory(k,_,r) if k.ticket==ticket=>Some(Stage::Volatile(r.clone())),
        State::Failed(k,_)|State::LostWindow(k) if k.ticket==ticket=>Some(Stage::Failed), _=>None,
    }
}

/// A launch that never obtained the primary claim must not start storage, plugins, terminal,
/// networking or the updater. This window has no document inputs or persistent callbacks.
pub fn unavailable() {
    *lock()=State::Fallback;
    rune::App::new(AppSpec::new(t!("rune.storage.memoryOnly.title"),(440.,140.),||(),|_,cx|{
        let p=crate::palette::pal(cx.system_dark());col().w_full().h_full().fill(p.bg_base).px(16.).py(12.).dir(crate::view::ui_direction()).child(text(t!("rune.storage.quit.unanswered")).color(p.text).wrap().w_full())
    })).args(Vec::new()).run();
}

/// We have no claim and have started no services. Keep the original arguments
/// in this launch; retry never steals the previous process's file ownership.
pub fn retry_closing()->bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDRETRY,MB_RETRYCANCEL,MessageBoxW};
    let reason:Vec<_>=t!("rune.storage.quit.handoffTimeout").encode_utf16().chain(Some(0)).collect();
    let title:Vec<_>="OpenCode".encode_utf16().chain(Some(0)).collect();
    // SAFETY: both strings are NUL-terminated for the entire modal call.
    #[allow(unsafe_code)]
    unsafe {MessageBoxW(std::ptr::null_mut(),reason.as_ptr(),title.as_ptr(),MB_RETRYCANCEL)==IDRETRY}
}
