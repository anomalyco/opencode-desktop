//! The window's frame: Rune's own, with what the app paints into effect slots at their places in
//! it. Office pages (`office::compositor`) and the new-session Easter egg (`egg::gpu`) each own
//! their marks. A frame with neither on screen is exactly Rune's default frame.

use rune::compose::Frame;

use crate::App;

#[derive(Default)]
pub struct AppCompositor {
    #[cfg(feature = "office")]
    office: crate::office::OfficeCompositor,
    egg: crate::egg::gpu::EggGpu,
}

impl rune::Compositor<App> for AppCompositor {
    fn draws_own(&self) -> bool {
        #[cfg(feature = "office")]
        if self.office.draws_own() {
            return true;
        }
        self.egg.draws_own()
    }

    fn render(&mut self, f: &mut Frame<'_, App>) {
        #[cfg(feature = "office")]
        let office = self.office.prepare(f);
        #[cfg(not(feature = "office"))]
        let office = false;
        let egg = self.egg.prepare(f);
        if !office && !egg {
            return f.draw_ui();
        }
        let clear = f.rt.clear_color();
        f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        let (queue, device) = (f.queue, f.device);
        #[cfg(feature = "office")]
        let pages = &mut self.office;
        let egg = &mut self.egg;
        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
            if crate::egg::gpu::is_mark(site.mark) {
                return egg.draw(pass, site, queue, device);
            }
            #[cfg(feature = "office")]
            pages.draw(pass, site, queue, device);
        });
    }

    fn animating(&self) -> bool {
        #[cfg(feature = "office")]
        if self.office.animating() {
            return true;
        }
        self.egg.animating()
    }
}
