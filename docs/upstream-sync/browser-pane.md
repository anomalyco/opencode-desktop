# Browser pane: official vs ours

Official's browser plugin read at `361bed51ef` (`packages/gui-plugins/src/browser/**`), ours at `dev` on 23 Nov 2026, feature by feature. Each row says where each side does it, and whether a person sees a difference.

- **Result:** no difference a person meets in the rows below; each official feature has its counterpart in ours, mostly ported line by line with official's file cited in ours.
- **Method:** every official `i18n/en.ts` key (412) was checked against what our code uses, not only against our string tables (ours carries official's whole string catalogue, so a string existing proves nothing). Every key no code of ours names turned out to be a renamed key (ours uses `rune.browser.*` for the same text) or one official itself never shows.
- **Not walked here:** pixel layout (the screen scoreboard covers it) and behaviour only an agent drives (`packages/plugin-browser`).

| Feature | Official | Ours | Same for a person |
| --- | --- | --- | --- |
| PDF, top level | Electron `WebContentsView` without `plugins: true` (`chromium.ts:350-363`): no PDF viewer, so the PDF downloads | engine `plugins.always_open_pdf_externally` (`context.rs:375-378`): the PDF goes to the Save dialog | yes: both download it |
| PDF in a frame | no viewer, Chromium's placeholder in the frame | no viewer; the placeholder navigation is cancelled, no bar since #770 | yes, the frame stays empty in both |
| Page context menu | `page-menu.ts`: back, forward, reload, hard reload, cut, copy, paste, paste as plain text, select all, spelling suggestions and add word, open / copy link, open / copy / save image, copy image address, open outside, spellcheck and languages | `apps/desktop/src/surfaces/browser/context_menu.rs`: the same items, grouped in official's order (navigation, open page, links, images, spelling, edit, spellcheck, extensions) | yes |
| Permission prompts | `prompts.tsx`, `prompt.permission.*` | `surfaces/browser/prompts/view.rs`, `rune.browser.prompt.perm.*` | yes |
| Pop-up blocked | `refused.popup` notice | engine `EngineMsg::PopupBlocked` → `prompts/view.rs:180-193`, with the 500 ms activation delay | yes |
| Refusal bar | `refusal-bar.tsx`, `failure.ts` codes | `plugins/browser/refusal.luau`, `failure.luau:refusalKey` | yes |
| Load failure page | `failure-view.tsx` | `failure.luau` + `panel.luau` | yes |
| Find in page | `find-bar.tsx`, `page-find.ts` | `plugins/browser/find.luau` | yes |
| Zoom | Chromium's zoom steps (`chromium.ts:181-182`) | `init.luau` `zoom` action, `tab:SetZoom` | yes |
| Site info | `site-*.ts`, drill-in views | `plugins/browser/site.luau` (focus between views: #717) | yes, except official keeps keyboard focus on the trigger when it opens; ours moves it into the popover |
| Viewport / device toolbar | `viewport-*.ts(x)` | `plugins/browser/device.luau` | yes |
| History, back / forward | `history/*`, navigation turn | `plugins/browser/history.luau`, engine navigation | yes |
| Sign-in offer, import | `import-prompt.tsx`, `site-chooser.tsx` | `signins.luau`, `settings.luau` (Choose sites: #711, #716) | yes |
| Extensions, review | `settings.tsx` review, `extensions.access.*`, `extensions.compat.*` | `plugins/browser/extensions.luau` (keys built at run time) | yes |
| Agent-driven markers | `agent-rendering.ts`, passkey and print locks | engine gates (`clipboard_gate.rs`, print gate, passkeys); first load after the gate script (#735) | yes |
