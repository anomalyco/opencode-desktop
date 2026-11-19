//! Shell kernel assembly: wires extracted feature crates and native surface descriptors into
//! `opencode_kernel::Kernel<App>`.
//!
//! New Rust features register here via `Feature::register` instead of adding fields to `App` or
//! editing central dispatch chains in `lib.rs`, `tick.rs`, `shell/input/keys.rs`, or `shell/view/mod.rs`.

use opencode_kernel::{
    AppEnv, AppFeedback, AppOverlays, EscapeLayer, Kernel, KernelBuilder, OverlayEntry,
    OverlayEvent, OverlayKind, OverlayRegistration, Phase, SurfaceDescriptor, SurfaceKind,
};

use crate::{App, Route};

impl AppEnv for App {
    fn clock(&self) -> f64 {
        *self.clock
    }

    fn now_ms(&self) -> i64 {
        *self.now_ms
    }

    fn zoom(&self) -> f32 {
        *self.zoom
    }

    fn spellcheck(&self) -> bool {
        self.settings.spellcheck
    }

    fn reduced_motion(&self) -> bool {
        *self.appearance_state.reduced_motion
    }

    fn is_live(&self) -> bool {
        self.live()
    }

    fn persists(&self) -> bool {
        App::persists(self)
    }

    fn is_headless(&self) -> bool {
        crate::headless()
    }

    fn waker(&self) -> Option<rune::Waker> {
        (*self.waker).clone()
    }

    fn active_session_id(&self) -> Option<&str> {
        match &*self.route {
            Route::Session { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }
}

impl AppFeedback for App {
    fn toast_say(&mut self, title: String, description: String) {
        crate::status_toast::say(self, title, description);
    }

    fn toast_succeed(&mut self, title: String, description: String) {
        crate::status_toast::succeed(self, title, description);
    }

    fn toast_fail(&mut self, title: String, description: String) {
        crate::status_toast::fail(self, title, description);
    }
}

impl AppOverlays for App {
    fn on_overlay_opened(&mut self, id: &'static str, return_focus: Option<String>) {
        let mut entry = OverlayEntry::new(id, OverlayKind::Dialog);
        if let Some(focus) = return_focus {
            entry = entry.with_return_focus(focus);
        }
        let _ = self.kernel.step_overlay(OverlayEvent::Push(entry));
    }

    fn on_overlay_closed(&mut self, id: &'static str) {
        let _ = self.kernel.step_overlay(OverlayEvent::Close(id));
    }
}

/// Builds a window's kernel with all decoupled features and core surfaces registered.
pub fn build() -> Kernel<App> {
    let mut b = KernelBuilder::new();
    register_core_systems(&mut b);
    b.with_feature(&crate::surfaces::egg::EggFeature);
    #[cfg(feature = "office")]
    b.with_feature(&crate::surfaces::office::OfficeFeature);
    b.with_feature(&crate::surfaces::updater::UpdaterFeature);
    b.with_feature(&crate::surfaces::code_view::CodeViewFeature);
    b.with_feature(&crate::line_comments::AnnotationsFeature);
    b.with_feature(&crate::attachments::ComposerInputFeature);
    b.with_feature(&crate::connect::ConnectFeature);
    b.with_feature(&crate::dictation::DictationFeature);
    b.with_feature(&crate::files::FilesFeature);
    b.with_feature(&crate::find::FindFeature);
    b.with_feature(&crate::message_nav::HistoryFeature);
    b.with_feature(&crate::queue::InboxFeature);
    b.with_feature(&crate::mcp::McpFeature);
    b.with_feature(&crate::mentions::MentionsFeature);
    b.with_feature(&crate::narrow::NarrowFeature);
    b.with_feature(&crate::notifications::NotificationsFeature);
    b.with_feature(&crate::open_in::OpenInFeature);
    b.with_feature(&crate::panels::PanelsFeature);
    b.with_feature(&crate::projects::ProjectsFeature);
    b.with_feature(&crate::restore::RestoreFeature);
    b.with_feature(&crate::review::ReviewFeature);
    b.with_feature(&crate::session_menu::SessionMenuFeature);
    b.with_feature(&crate::settings::SettingsFeature);
    b.with_feature(&crate::subagents::SubagentsFeature);
    b.with_feature(&crate::switcher::SwitcherFeature);
    b.with_feature(&crate::surfaces::terminal::TerminalSurfaceFeature);
    b.with_feature(&crate::todos::TimelineExtrasFeature);
    b.with_feature(&crate::tips::TipsFeature);
    b.with_feature(&crate::usage::UsageFeature);
    b.with_feature(&crate::new_worktree::WorkspaceToolsFeature);
    b.with_feature(&crate::plugin_host::PluginHostFeature);
    b.register_overlay(
        OverlayRegistration::<App>::new("overlay.diagram")
            .with_z(170)
            .with_key_priority(195)
            .with_render(|s, cx| {
                let p = crate::palette::pal(s.dark(cx));
                crate::diagram::view(s, p, cx)
            })
            .with_key(crate::diagram::key),
    );
    b.register_overlay(
        OverlayRegistration::<App>::new("dialog.move_recovery")
            .with_z(470)
            .with_key_priority(150)
            .with_render(|s, cx| {
                let p = crate::palette::pal(s.dark(cx));
                crate::view::move_recovery::view(s, p, cx)
            })
            .with_key(crate::view::move_recovery::key),
    );
    b.register_overlay(
        OverlayRegistration::<App>::new("dialog.servers")
            .with_z(490)
            .with_render(|s, cx| {
                let p = crate::palette::pal(s.dark(cx));
                crate::servers::dialog::view(s, p, cx)
            }),
    );
    b.register_escape_layer(EscapeLayer::new(
        "escape.tool_step",
        20,
        |s| s.tool_view_open(),
        |s, _, tx| {
            s.dismiss_tool_step(tx);
        },
    ));
    b.register_escape_layer(EscapeLayer::new(
        "escape.toast",
        30,
        |s| s.toast.current().is_some_and(|t| t.persistent),
        |s, ev, tx| {
            crate::status_toast::key(s, ev, tx);
        },
    ));
    b.register_escape_layer(EscapeLayer::new(
        "escape.opening",
        90,
        |s| s.opening.is_some(),
        |s, _, tx| {
            s.cancel_open(tx);
        },
    ));
    b.register_escape_layer(EscapeLayer::new(
        "escape.peek",
        120,
        |s| !*s.sidebar_open && s.layout.peek.shown(),
        |s, _, tx| {
            s.queue_peek(crate::layout_state::PeekEvent::Escape, tx);
        },
    ));
    crate::commands::register_shell_commands(&mut b);
    b.add_surface(
        SurfaceDescriptor::new("browser", SurfaceKind::Browser, "Browser")
            .with_multi_instance(true),
    );
    b.register_slot(&crate::app::VIEW_RUNS)
        .register_slot(&crate::app::OUTPUT_ERRORS)
        .register_slot(&crate::session_catalog::CATALOG)
        .register_slot(&crate::events::TAIL_CHANGE)
        .register_slot(&crate::events::STREAM_GAPS)
        .register_slot(&crate::speech::SPEECH)
        .register_slot(&crate::crash_notice::CRASH_NOTICE)
        .register_slot(&crate::servers::dialog::DIALOG);
    b.build()
}

/// Core shell and subsystem lifecycle polls registered at explicit `tick::order` positions so
/// exact four-phase execution order (`Ingest` -> `Derive` -> `React` -> `Persist`) never depends
/// on feature registration order.
fn register_core_systems(b: &mut KernelBuilder<App>) {
    use crate::tick::order::{derive, ingest, persist, react};

    b.add_visibility("status_toast.visibility", |s, shown, tx| {
        crate::status_toast::visibility(s, shown, tx);
    });

    // ── Phase 1: Ingest ──────────────────────────────────────────────────────────────────────────
    // Another window changed the session data this one shows.
    b.add_system_ordered(Phase::Ingest, ingest::WINDOWS_PEER_BEHIND, "windows.peer.behind", |s, _, _| {
        crate::windows::peer::behind(s)
    });
    b.add_system_ordered(Phase::Ingest, ingest::RESTORE_PLACES, "restore.places", |s, now, tx| {
        s.poll_places(now, tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::RESTORE_UNDO, "restore.undo", |s, now, tx| {
        s.poll_undo(now, tx);
        false
    });
    b.add_system_ordered(Phase::Ingest, ingest::RECONNECT_AUDIT, "reconnect.audit", |s, _, _| {
        crate::reconnect::audit(s);
        false
    });
    b.add_system_ordered(Phase::Ingest, ingest::POPUP, "popup.poll", |s, _, tx| {
        crate::popup::poll(s, tx);
        false
    });
    // The servers the window isn't checked out on: their launches, replies and streams.
    b.add_system_ordered(Phase::Ingest, ingest::SERVERS_RUNTIME, "servers.runtime", |s, _, tx| {
        crate::servers::runtime::poll(s, tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::SERVERS_HEALTH, "servers.health", |s, _, tx| {
        crate::servers::health::poll(s, tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::PROJECTS_HOME, "projects.home", |s, _, tx| {
        let fresh = s.tick_fresh();
        crate::projects::home::poll(s, fresh, tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::PROJECT_SWITCHER, "project_switcher.poll", |s, _, _| {
        crate::project_switcher::poll(s)
    });
    // A project's image the icon worker found (or lost): the tiles that read the icons draw again.
    b.add_system_ordered(Phase::Ingest, ingest::APPEARANCE_ICONS, "appearance.icons", |s, _, _| {
        if rune::Tracked::untracked(&s.appearance_state.icons).take_found() {
            rune::Tracked::touch(&s.appearance_state.icons);
            true
        } else {
            false
        }
    });
    b.add_system_ordered(Phase::Ingest, ingest::LAUNCH, "launch.poll", |s, _, tx| {
        crate::launch::poll(s, tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::LAUNCH_TRACE, "launch.trace", |_, _, tx| {
        crate::launch::trace::poll(tx);
        false
    });
    b.add_system_ordered(Phase::Ingest, ingest::GLITCH, "glitch.poll", |s, _, tx| {
        crate::glitch::poll(s, tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::APPEARANCE, "appearance.poll", |s, _, _| {
        crate::appearance::poll(s)
    });
    b.add_system_ordered(Phase::Ingest, ingest::TEXT_RENDERING, "text_rendering.poll", |s, _, tx| {
        crate::text_rendering::poll(s, tx);
        false
    });
    b.add_system_ordered(Phase::Ingest, ingest::LANGUAGE, "language.poll", |s, _, tx| {
        crate::language::poll(s, tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::SPEECH, "speech.poll", |s, _, tx| {
        crate::speech::poll(s, tx)
    });
    // A shortcut from the composer that moved the keyboard into the side region (its command
    // may run a frame later) leaves the selected tab's label muted (`FocusMode::Quiet`).
    b.add_system_ordered(Phase::Ingest, ingest::PANELS_SYNC_COMPOSER, "panels.sync_composer", |s, _, _| {
        let composing = s.composer.focused;
        let in_terminal = !composing
            && s.workspace_panes.side.focus.is_armed()
            && s.workspace_panes.side.focus.is_focused()
            && s.terminal_focus().is_some();
        let sync = crate::panels::SideFocusEvent::SyncComposer { composing, in_terminal };
        // Written only when it changes: a write to the side state rebuilds every region reading it.
        if s.workspace_panes.side.focus.would_change(sync) {
            s.workspace_panes.side.focus.sync_composer(composing, in_terminal);
        }
        false
    });
    b.add_system_ordered(Phase::Ingest, ingest::DEEP_LINKS, "deep_links.drain", |s, _, tx| {
        crate::deep_links::drain(s, tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::TURN_ERRORS, "turn_errors.poll", |s, _, tx| {
        crate::turn_errors::poll(s, tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::SESSION_PAGES, "session_pages.poll", |s, _, tx| {
        s.poll_pages(tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::APP_OUTPUT_ERRORS, "app.output_errors", |s, _, _| {
        let mut changed = false;
        while let Some(why) = s
            .kernel
            .slots
            .untracked(&crate::app::OUTPUT_ERRORS)
            .0
            .as_ref()
            .and_then(|(_, rx)| rx.try_recv().ok())
        {
            crate::status_toast::fail(s, opencode_i18n::t!("common.requestFailed"), why);
            changed = true;
        }
        changed
    });
    b.add_system_ordered(Phase::Ingest, ingest::CONTEXT_WINDOW, "context_window.poll", |s, _, _| {
        s.poll_context_window()
    });
    b.add_system_ordered(Phase::Ingest, ingest::INBOX_SENDS, "inbox.sends", |s, _, _| {
        s.poll_sends()
    });
    b.add_system_ordered(Phase::Ingest, ingest::INBOX_OUTBOX, "inbox.outbox", |s, _, tx| {
        s.tick_outbox(tx)
    });
    b.add_system_ordered(Phase::Ingest, ingest::JOBS, "jobs.poll", |s, _, tx| s.poll_jobs(tx));
    b.add_system_ordered(Phase::Ingest, ingest::TICK_STREAM, "tick.stream", crate::tick::poll_stream);
    b.add_system_ordered(Phase::Ingest, ingest::TICK_SUBMIT, "tick.submit", |s, _, tx| {
        crate::tick::poll_submit(s, tx)
    });

    // ── Phase 2: Derive ──────────────────────────────────────────────────────────────────────────
    b.add_system_ordered(Phase::Derive, derive::WORKING_SYNC, "working.sync", |s, _, _| {
        crate::working::sync(s)
    });
    b.add_system_ordered(Phase::Derive, derive::TIMELINE_DOCK_MORPH, "timeline.dock_morph", |s, _, tx| {
        let asked = s.take_asked_dock_face();
        if s.dock_face() != asked {
            s.morph(tx);
        }
        false
    });
    b.add_system_ordered(Phase::Derive, derive::DISCLOSURE, "disclosure.poll", crate::disclosure::poll);
    b.add_system_ordered(Phase::Derive, derive::TIMELINE_PROJECTION, "timeline.projection", |s, _, tx| {
        session_view::timeline::projection_poll(s, tx)
    });
    b.add_system_ordered(Phase::Derive, derive::LINE_REVEAL, "line_reveal.poll", |s, now, tx| {
        crate::line_reveal::poll(s, now, tx)
    });
    b.add_system_ordered(Phase::Derive, derive::COPIED, "copied.poll", crate::copied::poll);
    b.add_system_ordered(Phase::Derive, derive::DIAGRAM, "diagram.poll", crate::diagram::poll);
    b.add_system_ordered(Phase::Derive, derive::TIMELINE_IMAGES, "timeline.images", |s, now, tx| {
        session_view::images::poll(s, now, tx)
    });
    b.add_system_ordered(Phase::Derive, derive::MODEL_PICKER_HOVER, "model_picker.hover", |s, now, tx| {
        crate::model_picker::hover::poll(s, now, tx)
    });
    b.add_system_ordered(Phase::Derive, derive::BACKGROUND, "background.poll", |s, _, tx| {
        crate::background::poll(s, tx)
    });
    // Ingest local answers before settling the list: its rows, highlight and empty state must
    // all see the same sources. Previously a full-text landing changed the rows after this poll.
    b.add_system_ordered(Phase::Derive, derive::SWITCHER, "switcher.poll", |s, now, tx| {
        let local_answers = s.poll_session_search(now, tx) | s.poll_fulltext();
        s.poll_switcher(tx, local_answers)
    });
    b.add_system_ordered(Phase::Derive, derive::MENTIONS, "mentions.poll", |s, _, tx| {
        s.poll_mentions(tx)
    });

    // ── Phase 3: React ───────────────────────────────────────────────────────────────────────────
    b.add_system_ordered(Phase::React, react::COMPOSER_ATTACH, "composer_input.attach", |s, _, _| {
        if s.composer_ext.attach.poll() {
            rune::Tracked::touch(&s.composer_ext.attach);
            true
        } else {
            false
        }
    });
    b.add_system_ordered(Phase::React, react::PANELS, "panels.poll", |s, _, tx| {
        let fresh = s.tick_fresh();
        s.poll_panels(fresh, tx)
    });
    b.add_system_ordered(Phase::React, react::TURN_DIFFS, "turn_diffs.poll", |s, _, _| {
        s.poll_turn_diffs()
    });
    b.add_system_ordered(Phase::React, react::RECONNECT, "reconnect.poll", |s, _, tx| {
        crate::reconnect::poll(s, tx)
    });
    b.add_system_ordered(Phase::React, react::SESSION_STATE, "session_state.poll", |s, _, tx| {
        crate::session_state::poll(s, tx)
    });
    b.add_system_ordered(Phase::React, react::STATUS_TOAST, "status_toast.poll", |s, now, tx| {
        crate::status_toast::poll(s, now, tx)
    });
    b.add_system_ordered(Phase::React, react::LAYOUT_STATE, "layout_state.poll", |s, now, tx| {
        crate::layout_state::poll(s, now, tx)
    });
    b.add_system_ordered(Phase::React, react::INBOX_MODEL, "inbox_model.poll", |s, now, tx| {
        crate::inbox_model::poll(s, now, tx)
    });
    b.add_system_ordered(Phase::React, react::TAB_PREVIEW, "tab_preview.poll", |s, now, tx| {
        crate::view::tab_preview::poll(s, now, tx)
    });
    b.add_system_ordered(Phase::React, react::SESSION_STATUS, "session_status.poll", |s, now, tx| {
        crate::session_status::poll(s, now, tx)
    });
    // Each draft keeps its own composer in any window, saved or not (`restore::show_draft`).
    b.add_system_ordered(Phase::React, react::RESTORE_SHOW_DRAFT, "restore.show_draft", |s, _, _| {
        s.show_draft()
    });

    // ── Phase 4: Persist ─────────────────────────────────────────────────────────────────────────
    b.add_system_ordered(Phase::Persist, persist::RESTORE_PERSIST, "restore.persist", |s, _, tx| {
        if !s.persists() {
            return false;
        }
        let mut changed = false;
        changed |= s.tell_storage_notice();
        changed |= s.hydrate_restore();
        changed |= crate::crash_notice::poll(s);
        s.poll_restore();
        s.poll_preview(false);
        changed |= s.poll_copies(tx);
        changed
    });
    b.add_system_ordered(Phase::Persist, persist::ANNOTATIONS_SAVED, "annotations.poll_saved", |s, _, _| {
        s.persists() && crate::line_comments::poll_saved(s)
    });
    b.add_system_ordered(Phase::Persist, persist::WINDOWS_PERSIST, "windows.persist", |s, _, _| {
        if !s.persists() {
            return false;
        }
        crate::windows::reopen::poll(s);
        crate::windows::reopen::poll_asked(s);
        s.composer_ext.prompts.hydrate((*s.waker).clone());
        s.poll_window();
        s.poll_taskbar();
        crate::jump_list::poll(s);
        // A window with its own id saves its frame itself (`windows`), as AppKit's one autosave
        // name can't tell windows apart.
        if !*s.appearance_state.framed && s.window.id.is_empty() {
            *s.appearance_state.framed = rune_mac::remember_frame("OpenCodeRuneMain");
        }
        false
    });
}
