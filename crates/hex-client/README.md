# `hex-client`

Lightweight local HTTP client for driving [Hex](https://github.com/anomalyco/hex) voice dictation from Rune applications.

`dictate` runs one dictation over whatever the running Hex offers: its capture API (Hex records with the microphone the user chose in Hex and sends the words so far where its model streams), or, on a Windows Hex from before that API (`/capabilities` says no `serviceCapture`), a live `/streams` session where the app records the default microphone through WASAPI and Hex sends the words so far. It starts an installed Hex that isn't running, and says why it can't dictate with a fix: Hex's download, Hex's window, or the microphone privacy page.
