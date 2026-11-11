//! `ui.openInButton`: the app's Open in split button (`split-button-v2`, Desktop's
//! `OpenInAppButton`), from `crate::open_in`: the remembered app's mark opens the path in it, the
//! chevron lists the installed apps and Copy path. Nothing where opening isn't possible.

use rune::El;
use rune_luau::NodeArgs;

use super::p;
use crate::plugin_host::icons::intern;

pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    let app = crate::plugin_host::cx::app().ok_or("ui.openInButton draws only in a render")?;
    let key = intern(a.string("key").unwrap_or("open-in"));
    let path = a.string("path").unwrap_or_default().to_owned();
    Ok(crate::open_in::button(app, &p(a), key, &path).unwrap_or_else(rune::el))
}
