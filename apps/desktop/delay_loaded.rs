/// Windows: the static imports loaded on first use (`/DELAYLOAD`), with what calls them.
const DELAY_LOADED: &[&str] = &[
    "comctl32.dll",
    "uiautomationcore.dll",
    "wintrust.dll",
    "crypt32.dll",
    "setupapi.dll",
    "ws2_32.dll",
    "winmm.dll",
    "oleaut32.dll",
    "ole32.dll",
    "shell32.dll",
    "dwrite.dll",
    "dxgi.dll",
    "psapi.dll",
    // None of these is called before the first frame: the network and sleep notices are asked for
    // in `on_ready` (`launch::network`), the transacted rename is the updater's, the symbol reader
    // is `--launch-trace-io`'s, and std asks for the profile folder only when `USERPROFILE` is unset.
    // Mapped before `main` they cost 2,690 µs together (each loaded in the import table's order
    // by a program with no other imports, median of 15, a Windows dev box).
    "iphlpapi.dll",
    "powrprof.dll",
    "ktmw32.dll",
    "dbghelp.dll",
    "userenv.dll",
];
