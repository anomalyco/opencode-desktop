# `hex-client`

Lightweight local HTTP client for driving [Hex](https://github.com/anomalyco/hex) voice dictation from Rune applications.

`dictate` runs one dictation over whatever the running Hex offers: its capture API on macOS and Linux (Hex records), or a live `/streams` session on Windows, where the app records the default microphone through WASAPI and Hex sends the words so far. It starts an installed Hex that isn't running, and says why it can't dictate with a fix: Hex's download, Hex's window, or the microphone privacy page.
