fn main() {
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_WINDOW_TITLE");
}
