//! Images plugins draw: the ones a plugin hands over or loads (`Images`, `ui.image`). Each is
//! decoded by the window's Rune image cache on its own threads; a loaded one's bytes are read
//! first on the store's worker. Every image runs the [`load::Load`] machine; the host's poll
//! steps it, so a render only reads it.

pub mod load;

use std::collections::HashMap;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use rune::{Image, ImageCache};

use load::{Load, LoadEffect, LoadEvent, Source};

#[derive(Default)]
pub struct Images {
    cache: Option<ImageCache>,
    waker: Option<rune::Waker>,
    /// Id  the plugin it belongs to and where it stands.
    map: HashMap<u32, Entry>,
    /// A plugin's loads by source key, so the same source is the same image.
    by_source: HashMap<(String, String), u32>,
    next: u32,
    /// The worker's queue, started with the first load.
    jobs: Option<Sender<(u32, Source)>>,
    /// What the worker read, waiting for the poll.
    fetched: Arc<Mutex<Vec<Fetched>>>,
    /// The videos plugins show (`ui.video`), by plugin and URL: one player each.
    players: HashMap<(String, String), rune::Player>,
}

/// An image's id and what the worker read for it.
type Fetched = (u32, Result<Vec<u8>, String>);

struct Entry {
    plugin: String,
    load: Load,
}

impl Images {
    /// Starts the cache once the window's waker exists; a decoded image wakes it.
    pub fn start(&mut self, waker: rune::Waker) {
        self.cache.get_or_insert_with(|| ImageCache::new(waker.clone()));
        self.waker.get_or_insert(waker);
    }

    /// Plugin `plugin`'s image from `bytes`: encoded when `width` is 0, else RGBA8. 0 when refused.
    pub fn add(&mut self, plugin: &str, bytes: &[u8], width: u32, height: u32) -> u32 {
        let Some(cache) = &self.cache else { return 0 };
        if bytes.is_empty() || (width > 0 && bytes.len() != width as usize * height as usize * 4) {
            return 0;
        }
        let bytes: Arc<[u8]> = bytes.into();
        let image = if width == 0 { cache.load(bytes.clone()) } else { cache.rgba(width, height, bytes.clone()) };
        self.insert(plugin, Load::Decoding { image, bytes })
    }

    /// Plugin `plugin`'s SVG image, rasterized at twice its own size.
    pub fn add_svg(&mut self, plugin: &str, svg: &str) -> u32 {
        let Some(cache) = &self.cache else { return 0 };
        let bytes: Arc<[u8]> = svg.as_bytes().into();
        let image = cache.load(bytes.clone());
        self.insert(plugin, Load::Decoding { image, bytes })
    }

    /// Plugin `plugin`'s image from `source`, known by `key`: the image it loaded from there
    /// before (loading, loaded or failed), else a new one its worker starts reading.
    pub fn load(&mut self, plugin: &str, key: String, source: impl FnOnce() -> Option<Source>) -> u32 {
        if let Some(id) = self.by_source.get(&(plugin.to_owned(), key.clone())) {
            return *id;
        }
        let id = self.insert(plugin, Load::Fetching);
        self.by_source.insert((plugin.to_owned(), key), id);
        let sent = match (source(), self.jobs()) {
            (Some(source), Some(jobs)) => jobs.send((id, source)).map_err(|_| "the image reader stopped"),
            (None, _) => Err("no such image source"),
            (_, None) => Err("this window loads no images"),
        };
        if let Err(why) = sent {
            self.step(id, LoadEvent::Fetched(Err(why.into())));
        }
        id
    }

    fn insert(&mut self, plugin: &str, load: Load) -> u32 {
        self.next += 1;
        self.map.insert(self.next, Entry { plugin: plugin.to_owned(), load });
        self.next
    }

    /// The worker that reads sources, started on first use; `None` before the window's waker.
    fn jobs(&mut self) -> Option<Sender<(u32, Source)>> {
        if let Some(jobs) = &self.jobs {
            return Some(jobs.clone());
        }
        let waker = self.waker.clone()?;
        let (jobs, receive) = channel::<(u32, Source)>();
        let fetched = self.fetched.clone();
        // A worker that can't start drops its queue, so what's sent to it fails.
        let _worker = std::thread::Builder::new().name("plugin-images".into()).spawn(move || {
            while let Ok((id, source)) = receive.recv() {
                let read = source.read();
                fetched.lock().unwrap_or_else(|e| e.into_inner()).push((id, read));
                waker();
            }
        });
        self.jobs = Some(jobs.clone());
        Some(jobs)
    }

    /// Steps every image whose bytes landed or whose decode settled; true when what a render
    /// reads moved. At rest it does nothing.
    pub fn poll(&mut self) -> bool {
        let fetched = std::mem::take(&mut *self.fetched.lock().unwrap_or_else(|e| e.into_inner()));
        let mut changed = false;
        for (id, read) in fetched {
            let event = LoadEvent::Fetched(match (read, &self.cache) {
                (Ok(bytes), Some(cache)) => {
                    let bytes: Arc<[u8]> = bytes.into();
                    Ok((cache.load(bytes.clone()), bytes))
                }
                (Ok(_), None) => Err("this window decodes no images".into()),
                (Err(e), _) => Err(e),
            });
            changed |= self.step(id, event);
        }
        let settled: Vec<u32> = self.map.iter().filter(|(_, e)| matches!(&e.load, Load::Decoding { image, .. } if image.is_ready())).map(|(id, _)| *id).collect();
        for id in settled {
            changed |= self.step(id, LoadEvent::Decoded);
        }
        changed
    }

    fn step(&mut self, id: u32, event: LoadEvent) -> bool {
        let Some(entry) = self.map.get_mut(&id) else { return false };
        let (next, effect) = std::mem::replace(&mut entry.load, Load::Fetching).step(event);
        entry.load = next;
        effect == Some(LoadEffect::Changed)
    }

    /// Plugin `plugin`'s image `id`, if it has one by that id.
    pub fn get(&self, plugin: &str, id: u32) -> Option<&Load> {
        self.map.get(&id).filter(|e| e.plugin == plugin).map(|e| &e.load)
    }

    pub fn remove(&mut self, plugin: &str, id: u32) {
        if self.map.get(&id).is_some_and(|e| e.plugin == plugin) {
            self.map.remove(&id);
            self.by_source.retain(|_, v| *v != id);
        }
    }

    /// Forgets everything plugin `plugin` handed over, loaded or played (it stopped).
    pub fn forget(&mut self, plugin: &str) {
        self.map.retain(|_, e| e.plugin != plugin);
        self.by_source.retain(|(p, _), _| p != plugin);
        self.players.retain(|(p, _), _| p != plugin);
    }

    /// Plugin `plugin`'s player of video `url`, made playing muted on a loop (`autoplay loop
    /// muted`) the first time it shows; it plays only while an element draws it.
    pub fn player(&mut self, plugin: &str, url: &str) -> rune::Player {
        self.players
            .entry((plugin.to_owned(), url.to_owned()))
            .or_insert_with(|| {
                let player = rune::Player::url(url);
                player.set_muted(true);
                player.set_looping(true);
                player.play();
                player
            })
            .clone()
    }
}

/// An image handle for the viewer, with the bytes its menu saves.
pub fn shown(load: &Load) -> Option<(Image, Arc<[u8]>)> {
    Some((load.image()?.clone(), load.bytes()?.clone()))
}
