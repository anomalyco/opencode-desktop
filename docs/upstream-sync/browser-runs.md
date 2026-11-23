# Browser runs: official vs ours

The same pages, run in official's real desktop app and in our browser engine, and what a person gets in each. Unlike `browser-pane.md` (read from the code), every row here was run.

- **Official:** the desktop app from `anomalyco/opencode` `dev` (Electron 44.9.1, Chromium 152), started with a scratch profile. The pages load in its browser pane, and a CDP client reads their results (`proof/ws1/runs/official-*.json`).
- **Ours:** the browser engine from `dev`, started with a scratch profile and every user folder sealed, on a person's tab (`proof/ws1/runs/ours-*.json`).
- **Pages:** local test pages on loopback (`caps`, `play`, `store`, `net`). The media files are 2-second samples made with `ffmpeg`.
- **Machine:** Windows 11, Intel Iris Xe.

| Area | Feature | Official | Ours | PR |
| --- | --- | --- | --- | --- |
| Media | H.264 + AAC MP4 | plays | **does not play** (`DEMUXER_ERROR_NO_SUPPORTED_STREAMS`) | open: needs a CEF build with proprietary codecs |
| Media | HEVC MP4 | plays | **does not play** | open: same |
| Media | AAC audio (`.m4a`) | plays | **does not play** | open: same |
| Media | HLS (`.m3u8`, H.264) | plays | **does not play** | open: same |
| Media | VP8, VP9, AV1 (WebM and MP4), Opus, Vorbis, MP3, FLAC, WAV | plays | plays | |
| Media | Captions (`<track>` WebVTT) | loads | loads | |
| Media | Widevine and PlayReady DRM | not offered (`NotSupportedError`) | not offered | |
| Media | Picture-in-picture, fullscreen | enabled | enabled | |
| Media | Sound without a click first | plays | plays | #807 |
| Graphics | WebGL 1 / 2, renderer | ANGLE D3D11, the machine's GPU | the same | |
| Graphics | WebGPU adapter | the machine's GPU | the same | |
| Graphics | Colour emoji font, canvas | present | present | |
| Documents | PDF, top level and in a frame | shown inline | shown inline | #807 |
| Identity | Platform authenticator (passkeys) | available | available | |
| Storage | localStorage, IndexedDB, Cache API, cookies across an app restart | kept | kept | |
| Storage | Service worker registers and activates | yes | yes | |
| Network | WebSocket, server-sent events, a 3-step redirect, a 32 MB fetch | work | work | |
| APIs | `navigator.share` | absent | **was present** (Windows share sheet), now absent | this PR |
| APIs | Screen wake lock | permission `denied` | permission `granted` | open: setting its content setting hangs CEF 156 pages |
| APIs | Storage Access API | permission `denied` | permission `granted` | open: same |
| Notifications | A site the person allowed | system notification under the app's name | system notification through the app's sink | #809 |
| Permissions | `navigator.permissions.query` for location, notifications, camera, microphone and clipboard read on a site not yet asked | `denied` (a request still prompts) | `prompt` (a request prompts) | kept on purpose: `notes/upstream-sync/ours.md` |

## Not run yet

- Forms and input: file upload, dropping a file, spellcheck suggestions, IME, autofill, date / colour / select pickers, context menus, copy and paste.
- Navigation and windows: `target=_blank`, `window.open`, `beforeunload`, alert / confirm / prompt, basic auth, certificate errors, mixed content, `mailto:` / `tel:`, each kind of download, view-source, find.
- Network: HTTP/2 and HTTP/3, proxy settings, error pages.
