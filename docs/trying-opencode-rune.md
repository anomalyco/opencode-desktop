# Trying OpenCode (Rune)

**OpenCode (Rune)** is the native macOS desktop client (`apps/desktop`) built on Rune. You can keep the `main` build installed for daily work and drop pull-request builds next to it whenever you want to test a change.

## 1. Install the `main` build

1. Download [`OpenCode (Rune).dmg`](https://anoma.ly/release/opencode-rune/desktop/dev/macos-arm64-dmg) (Apple Silicon, macOS 26 or later) and drag **OpenCode (Rune).app** into `/Applications`.
2. Make sure an OpenCode server is running locally, either by opening **OpenCode Desktop** or by running `opencode serve --port 4096` in a terminal.
3. Open **OpenCode (Rune)**. It connects to `http://127.0.0.1:4096` or `~/.config/opencode/service.json` automatically, and it never starts a server on its own. If nothing is running yet, start the server and click **Retry**.

The `main` app checks `https://anoma.ly/release` 10 seconds after launch and once an hour. When a new `main` commit lands in `apps/desktop`, the app downloads and verifies the update in the background and switches to it on your next restart. Opening the browser pane (`⌥⌘B`) downloads the Browser Engine sidecar from `https://anoma.ly/release` without GitHub sign-in. The app also fetches it on its own once it has been idle for 30 seconds (throttled, and resumed if cut short), so the pane usually opens without a wait. Turn that off in **Settings › Download browser in the background**.

## 2. Try a pull request side by side

Every open pull request in `anomalyco/rune-desktop` that touches `apps/desktop` (or one of its crates) posts a comment with a link to `OpenCode-Rune-PR-<N>-macos-arm64.dmg`.

1. Download the `.dmg` from the PR comment and drag **`OpenCode (Rune) · <branch>.app`** to `/Applications` (or open it directly from the mounted disk image).
2. Look for the `#<N>` badge on its Dock icon and the branch name in its title. Each PR build has its own bundle ID (`anomaly.opencode-rune.pr.<N>`) and does not auto-update, so your `/Applications/OpenCode (Rune).app` from `main` stays untouched.
3. Switch between **`OpenCode (Rune)`** and **`OpenCode (Rune) · <branch>`** with `⌘Tab` or the Dock. Both apps talk to the same local OpenCode server, so you can compare the same session in both builds.
4. Drag **`OpenCode (Rune) · <branch>.app`** to the Trash when you finish testing. GitHub Actions deletes the hosted PR build automatically as soon as the PR merges or closes.

## 3. Send feedback with Scrawl

Scrawl records your screen, voice, cursor clicks, and drawn marks into a bundle an agent or teammate can act on right away.

1. Install Scrawl once from [scrawl.kitlangton.com](https://scrawl.kitlangton.com) or with Homebrew:
   ```sh
   brew install --cask kitlangton/tap/scrawl
   ```
2. Focus the **OpenCode (Rune)** window you are testing and press `⌥⇧S` to start recording (or hold `⌥⇧S` to pick a single window).
3. Show the issue, draw on the part that looks wrong, explain what you expected out loud, and press `⌥⇧S` again to finish.
4. From Scrawl's review window, copy the capture or export an `.mp4`, then paste it on the PR thread or post it in `#abattoir` with the PR number (shown on the Dock icon badge).
