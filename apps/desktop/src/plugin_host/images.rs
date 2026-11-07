//! Images plugins hand the host (`HostApiVtable::image`): copied in, decoded by Rune's image
//! cache on its own worker, and drawn as `ElKind::Image` by id until the plugin drops them or
//! unloads.

use std::collections::HashMap;

use rune::{El, Image, ImageCache, SurfaceFit};

#[derive(Default)]
pub struct Images {
    cache: Option<ImageCache>,
    /// Id → the plugin that made it and the image.
    map: HashMap<u32, (String, Image)>,
    next: u32,
}

impl Images {
    /// Starts the cache once the window's waker exists; a decoded image wakes it.
    pub fn start(&mut self, waker: rune::Waker) {
        self.cache.get_or_insert_with(|| ImageCache::new(waker));
    }

    /// Plugin `plugin`'s image from `bytes`: encoded when `width` is 0, else RGBA8. 0 when refused.
    pub fn add(&mut self, plugin: &str, bytes: &[u8], width: u32, height: u32) -> u32 {
        let Some(cache) = &self.cache else { return 0 };
        if bytes.is_empty() {
            return 0;
        }
        let image = if width == 0 {
            cache.load(bytes.to_vec())
        } else {
            if bytes.len() != width as usize * height as usize * 4 {
                return 0;
            }
            cache.rgba(width, height, bytes.to_vec())
        };
        self.next += 1;
        self.map.insert(self.next, (plugin.to_owned(), image));
        self.next
    }

    pub fn remove(&mut self, plugin: &str, id: u32) {
        if self.map.get(&id).is_some_and(|(p, _)| p == plugin) {
            self.map.remove(&id);
        }
    }

    /// Forgets everything plugin `plugin` handed over (it unloaded).
    pub fn forget(&mut self, plugin: &str) {
        self.map.retain(|_, (p, _)| p != plugin);
    }

    /// Image `id` of plugin `plugin`, fitted inside `el`'s box.
    pub fn draw(&self, plugin: &str, id: u32, el: El) -> El {
        match self.map.get(&id).filter(|(p, _)| p == plugin) {
            Some((_, image)) => el.pixels_fit(image.pixels(), SurfaceFit::Fit),
            None => el,
        }
    }
}
