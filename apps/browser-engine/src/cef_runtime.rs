//! CEF 156 runtime initialization, browser command-line sanitization, and global
//! `RequestContextHandler` attachment (`DefaultRequestContextHandler`).

use std::sync::mpsc::Sender;

use cef::rc::Rc as _;
use cef::{
    App, BrowserProcessHandler, CefString, CommandLine, ImplApp, ImplBrowserProcessHandler,
    ImplCommandLine, RequestContextHandler, SchemeRegistrar, Settings, WrapApp,
    WrapBrowserProcessHandler, sys, wrap_app, wrap_browser_process_handler,
};

use crate::profile::Profile;

pub struct CefRuntime {
    profile: Profile,
    #[cfg(target_os = "macos")]
    _loader: cef::library_loader::LibraryLoader,
}

impl CefRuntime {
    /// `adapter_luid`: the app's GPU (`HighPart`, `LowPart`), for Chromium to render on;
    /// `locale`: the app's language as Chromium names it; `pump`: where CEF's requests for its
    /// next work call go.
    pub fn initialize(
        profile: Profile,
        adapter_luid: Option<(i32, u32)>,
        locale: Option<&str>,
        ext_dirs: &[std::path::PathBuf],
        io_pipes: Option<String>,
        pump: Sender<crate::Event>,
    ) -> Self {
        #[cfg(target_os = "macos")]
        crate::mac_app::ensure_ns_application();

        #[cfg(target_os = "macos")]
        let loader = {
            let exe = std::env::current_exe().expect("current_exe");
            let loader = cef::library_loader::LibraryLoader::new(&exe, false);
            assert!(loader.load(), "failed to load Chromium Embedded Framework");
            loader
        };

        let _ = cef::api_hash(sys::CEF_API_VERSION_LAST, 0);

        #[cfg(unix)]
        let exe_str = std::env::current_exe()
            .ok()
            .and_then(|p| p.into_os_string().into_string().ok())
            .unwrap_or_else(|| "opencode-browser-engine".into());
        #[cfg(unix)]
        let c_exe = std::ffi::CString::new(exe_str).unwrap();
        #[cfg(unix)]
        let argv = [c_exe.as_ptr()];
        #[cfg(unix)]
        let main_args = cef::MainArgs { argc: 1, argv: argv.as_ptr() as *mut *mut _ };
        #[cfg(windows)]
        let main_args = cef::args::Args::new().as_main_args().clone();

        // macOS finds its helper apps by bundle convention; elsewhere CEF would start this
        // executable again for its renderer and GPU processes, so point it at the helper.
        #[cfg(not(target_os = "macos"))]
        let browser_subprocess_path = std::env::current_exe()
            .ok()
            .map(|exe| exe.with_file_name(format!("opencode-browser-engine-helper{}", std::env::consts::EXE_SUFFIX)))
            .map(|helper| CefString::from(helper.to_string_lossy().as_ref()))
            .unwrap_or_default();
        #[cfg(target_os = "macos")]
        let browser_subprocess_path = CefString::default();

        let ext_except = (!ext_dirs.is_empty()).then(|| {
            ext_dirs
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(",")
        });
        let mut app = SidecarCefApp::new(
            adapter_luid.map(|(high, low)| format!("{high},{low}")),
            ext_except,
            io_pipes,
            pump,
        );
        let settings = Settings {
            browser_subprocess_path,
            windowless_rendering_enabled: 1,
            external_message_pump: 1,
            no_sandbox: 1,
            root_cache_path: CefString::from(profile.root.to_string_lossy().as_ref()),
            cache_path: CefString::from(profile.cache.to_string_lossy().as_ref()),
            persist_session_cookies: 1,
            log_severity: sys::cef_log_severity_t::LOGSEVERITY_WARNING.into(),
            // In the profile, not beside the executable (CEF's default), whose folder is the install.
            log_file: CefString::from(profile.root.join("debug.log").to_string_lossy().as_ref()),
            // Without one, Chromium follows the system's language. Pages hear the same language
            // first (Accept-Language, navigator.languages), then English.
            locale: locale.map(CefString::from).unwrap_or_default(),
            accept_language_list: locale
                .map(|l| if l.starts_with("en") { format!("{l},en") } else { format!("{l},en-US,en") })
                .map(|list| CefString::from(list.as_str()))
                .unwrap_or_default(),
            ..Default::default()
        };
        let ok = cef::initialize(Some(&main_args), Some(&settings), Some(&mut app), std::ptr::null_mut());
        assert_eq!(ok, 1, "cef::initialize failed");
        if let Some(global) = cef::request_context_get_global_context() {
            crate::preview::handler::ban_file_scheme(&global);
        }

        Self {
            profile,
            #[cfg(target_os = "macos")]
            _loader: loader,
        }
    }

    /// The profile and sibling isolated-downloads roots for `Downloads::open`.
    pub fn download_roots(&self) -> crate::downloads::DownloadRoots {
        crate::downloads::DownloadRoots {
            profile: self.profile.root.clone(),
            lasting_profile: self.profile.lasting_root.clone(),
            isolated_base: self.profile.isolated_downloads_base.clone(),
        }
    }
}

impl Drop for CefRuntime {
    fn drop(&mut self) {
        cef::shutdown();
        // The pages' downloads end with the engine, as Desktop's go with its pages.
        crate::downloads::remove_owned(&self.profile.root);
        crate::downloads::remove_owned_isolated(&self.profile.isolated_downloads_base, std::process::id());
        if self.profile.temporary {
            let _ = std::fs::remove_dir_all(&self.profile.root);
        }
    }
}

wrap_app! {
    struct SidecarCefApp {
        // Chromium's `use-adapter-luid` (`HIGH,LOW`): the GPU the app's frames open on.
        adapter_luid: Option<String>,
        // Managed user-approved extension directories (`--disable-extensions-except`), or None to
        // pass `--disable-extensions`. Never allows host registry or external loaders.
        ext_except: Option<String>,
        // Windows only: anonymous non-inheritable pipe HANDLE pair for `--remote-debugging-pipe`.
        io_pipes: Option<String>,
        pump: Sender<crate::Event>,
    }
    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&cef::CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cmd) = command_line else { return };
            // Strip untrusted external switches on the browser process (`GetCommandLineW()` passes
            // through on Windows; `review-rd274.md` §7) before appending our controlled flags.
            let is_browser_process = process_type.is_none_or(|p| p.to_string().is_empty());
            if is_browser_process {
                for sw in [
                    "remote-debugging-port",
                    "remote-debugging-pipe",
                    "remote-debugging-io-pipes",
                    "load-extension",
                    "load-component-extension",
                    "disable-extensions-except",
                    "disable-extensions",
                    "disable-web-security",
                    "allow-file-access-from-files",
                    "user-data-dir",
                    "disable-site-isolation-trials",
                    "proxy-server",
                    "proxy-pac-url",
                    "proxy-bypass-list",
                    "host-resolver-rules",
                    "ignore-certificate-errors",
                    "use-mock-keychain",
                    "password-store",
                    "unsafely-treat-insecure-origin-as-secure",
                    "js-flags",
                    "renderer-cmd-prefix",
                    "utility-cmd-prefix",
                ] {
                    cmd.remove_switch(Some(&sw.into()));
                }
            }
            // Rune owns the window. CEF paints offscreen even when no display server is running.
            #[cfg(target_os = "linux")]
            crate::linux_gpu::configure(cmd);
            // Keep Chrome's host registry, component background extensions, & external loaders off:
            // with 0 user-approved extensions pass `--disable-extensions`; with >= 1 pass
            // `--disable-extensions-except=<inert_stub_dirs>` plus our private in-process CDP pipe.
            cmd.append_switch(Some(&"disable-component-extensions-with-background-pages".into()));
            if let Some(dirs) = &self.ext_except {
                cmd.append_switch_with_value(Some(&"disable-extensions-except".into()), Some(&dirs.as_str().into()));
                cmd.append_switch(Some(&"remote-debugging-pipe".into()));
                if let Some(pipes) = &self.io_pipes {
                    cmd.append_switch_with_value(Some(&"remote-debugging-io-pipes".into()), Some(&pipes.as_str().into()));
                }
            } else {
                cmd.append_switch(Some(&"disable-extensions".into()));
            }
            // Let `TabRequestHandler::auth_credentials` handle HTTP 401 challenges in OSR instead
            // of Chrome's native login dialog (which fails with `ERR_INVALID_AUTH_CREDENTIALS` on
            // windowless browsers; CEF issue #3603).
            cmd.append_switch(Some(&"disable-chrome-login-prompt".into()));
            // Route script-initiated pop-ups through `TabLifeSpanHandler::on_before_popup` (which
            // always returns `1` to cancel native windows and holds `user_gesture == 0` in the
            // host's "Pop-up blocked" bar) instead of silently swallowing them before CEF sees them.
            cmd.append_switch(Some(&"disable-popup-blocking".into()));
            cmd.append_switch(Some(&"mute-audio".into()));
            // Windows keeps real DPAPI cookie encryption (silent, logon-bound).
            // macOS restores `--use-mock-keychain` until the `-67030` process-requirement /
            // dedicated Keychain service name fix lands in a follow-up PR, so the first load never
            // raises a `SecurityAgent` prompt or stalls.
            // Linux sets `--password-store=basic` so a locked or absent Secret Service / KWallet
            // keyring can never prompt or stall the first load.
            #[cfg(target_os = "macos")]
            cmd.append_switch(Some(&"use-mock-keychain".into()));
            #[cfg(target_os = "linux")]
            cmd.append_switch_with_value(Some(&"password-store".into()), Some(&"basic".into()));
            cmd.append_switch(Some(&"no-startup-window".into()));
            cmd.append_switch(Some(&"noerrdialogs".into()));
            cmd.append_switch(Some(&"hide-crash-restore-bubble".into()));
            // A side panel's pages, as Desktop's Electron shows them: no component downloads
            // (Widevine, Safe Browsing lists, on-device models), no field-trial or other
            // background fetches, which cost network, disk and CPU while nothing is open.
            cmd.append_switch(Some(&"disable-component-update".into()));
            cmd.append_switch(Some(&"disable-background-networking".into()));
            if let Some(luid) = &self.adapter_luid {
                cmd.append_switch_with_value(Some(&"use-adapter-luid".into()), Some(&luid.as_str().into()));
            }
        }

        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            crate::preview::scheme::register(registrar);
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(SidecarProcessHandler::new(self.pump.clone()))
        }
    }
}

wrap_browser_process_handler! {
    struct SidecarProcessHandler {
        pump: Sender<crate::Event>,
    }
    impl BrowserProcessHandler {
        // Any thread: the main loop wakes for it.
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            let _ = self.pump.send(crate::Event::Pump(delay_ms));
        }

        // Attach our global RequestContextHandler so extension service workers, offscreen
        // documents, and extension-origin initiators can never fetch or redirect to `file://`.
        fn default_request_context_handler(&self) -> Option<RequestContextHandler> {
            Some(crate::context::DefaultRequestContextHandler::new())
        }
    }
}
