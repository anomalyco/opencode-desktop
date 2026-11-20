//! The window's frame: Rune's own, with what the app paints into effect slots at their places in
//! it. Office pages (`office::compositor`), the new-session Easter egg (`egg::gpu`) and agent
//! widgets' shaders (`widgets::shader::gpu`) each own their marks. A frame with neither on screen is exactly Rune's default frame.

use rune::compose::Frame;

use crate::App;

#[derive(Default)]
pub struct AppCompositor {
    #[cfg(feature = "office")]
    office: crate::office::OfficeCompositor,
    egg: crate::egg::gpu::EggGpu,
    shaders: crate::session::widgets::shader::gpu::ShaderGpu,
}

impl rune::Compositor<App> for AppCompositor {
    fn draws_own(&self) -> bool {
        #[cfg(feature = "office")]
        if self.office.draws_own() {
            return true;
        }
        self.egg.draws_own() || self.shaders.draws_own()
    }

    fn render(&mut self, f: &mut Frame<'_, App>) {
        #[cfg(feature = "office")]
        let office = self.office.prepare(f);
        #[cfg(not(feature = "office"))]
        let office = false;
        let egg = self.egg.prepare(f);
        let shaders = self.shaders.prepare(f);
        if !office && !egg && !shaders {
            return f.draw_ui();
        }
        let clear = f.rt.clear_color();
        f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        let (queue, device) = (f.queue, f.device);
        #[cfg(feature = "office")]
        let pages = &mut self.office;
        let egg = &mut self.egg;
        let widgets = &mut self.shaders;
        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
            if crate::egg::gpu::is_mark(site.mark) {
                egg.draw(pass, site, queue, device);
            } else if crate::session::widgets::shader::is_mark(site.mark) {
                widgets.draw(pass, site, queue, device);
            } else {
                #[cfg(feature = "office")]
                pages.draw(pass, site, queue, device);
            }
        });
    }

    fn animating(&self) -> bool {
        #[cfg(feature = "office")]
        if self.office.animating() {
            return true;
        }
        self.egg.animating() || self.shaders.animating()
    }
}
