//! Shell kernel assembly: wires extracted feature crates and native surface descriptors into
//! `opencode_kernel::Kernel<App>`.
//!
//! New Rust features register here via `Feature::register` instead of adding fields to `App` or
//! editing central dispatch chains in `lib.rs`, `tick.rs`, `shell/input/keys.rs`, or `shell/view/mod.rs`.

use opencode_kernel::{
    AppEnv, AppFeedback, AppOverlays, EscapeLayer, Kernel, KernelBuilder, OverlayEntry,
    OverlayEvent, OverlayKind, OverlayRegistration, SurfaceDescriptor, SurfaceKind,
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
    b.add_stages(crate::tick::stage::ALL);
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
        |s| s.dialogs.toast.current().is_some_and(|t| t.persistent),
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
    let mut kernel = b.build();
    kernel.slots.insert(crate::app::ViewRuns::default());
    kernel.slots.insert(crate::session_catalog::Catalog::default());
    kernel
}

/// Core shell and subsystem lifecycle polls registered into declared [`crate::tick::stage`]
/// barriers with explicit `after` dependencies so execution order never depends on feature
/// registration order.
fn register_core_systems(b: &mut KernelBuilder<App>) {
    use crate::tick::stage;

    b.add_visibility("status_toast.visibility", |s, shown, tx| {
        crate::status_toast::visibility(s, shown, tx);
    });

    // ── Phase 1: Ingest ──────────────────────────────────────────────────────────────────────────
    // Another window changed the session data this one shows.
    b.schedule(stage::INGEST_HYDRATE, "windows.peer.behind", |s, _, _| {
        crate::windows::peer::behind(s)
    });
    b.schedule(stage::INGEST_HYDRATE, "restore.places", |s, now, tx| {
        s.poll_places(now, tx)
    });
    // Precaution: both `restore.places` and `restore.undo` write to restore state and storage.
    b.schedule(stage::INGEST_HYDRATE, "restore.undo", |s, now, tx| {
        s.poll_undo(now, tx);
        false
    })
    .after("restore.places");
    b.schedule(stage::INGEST_HYDRATE, "reconnect.audit", |s, _, _| {
        crate::reconnect::audit(s);
        false
    });
    b.schedule(stage::INGEST_HYDRATE, "popup.poll", |s, _, tx| {
        crate::popup::poll(s, tx);
        false
    });

    // The servers the window isn't checked out on: their launches, replies and streams.
    b.schedule(stage::INGEST_SERVERS, "servers.runtime", |s, _, tx| {
        crate::servers::runtime::poll(s, tx)
    });
    // Known: `servers.runtime::sync` creates controllers before `servers.health` calls `runtime::reported`.
    b.schedule(stage::INGEST_SERVERS, "servers.health", |s, _, tx| {
        crate::servers::health::poll(s, tx)
    })
    .after("servers.runtime");
    // Known/precaution: reads `checkout`, `servers::runtime::sidecar_stamp`, and listed server state.
    b.schedule(stage::INGEST_SERVERS, "projects.home", |s, _, tx| {
        crate::projects::home::poll(s, tx)
    })
    .after("servers.health");
    // Precaution: reads listed server projects/worktrees; `projects.home` can open a project dir.
    b.schedule(stage::INGEST_SERVERS, "project_switcher.poll", |s, _, _| {
        crate::project_switcher::poll(s)
    })
    .after("projects.home");
    // Precaution: `launch.poll` mutates `source`, `tabs`, `route`, `sessions`, `projects`, and `transcripts`.
    b.schedule(stage::INGEST_SERVERS, "launch.poll", |s, _, tx| {
        crate::launch::poll(s, tx)
    })
    .after("dictation.poll");
    // Known: `launch.trace` observes `rune::marks` emitted by `launch.poll`.
    b.schedule(stage::INGEST_SERVERS, "launch.trace", |_, _, tx| {
        crate::launch::trace::poll(tx);
        false
    })
    .after("launch.poll");

    // A project's image the icon worker found (or lost): the tiles that read the icons draw again.
    b.schedule(stage::INGEST_LOCAL, "appearance.icons", |s, _, _| {
        if rune::Tracked::untracked(&s.appearance_state.icons).take_found() {
            rune::Tracked::touch(&s.appearance_state.icons);
            true
        } else {
            false
        }
    });
    b.schedule(stage::INGEST_LOCAL, "glitch.poll", |s, _, tx| {
        crate::glitch::poll(s, tx)
    });
    // Precaution: `egg.poll` reads `reduced_motion`, which `appearance.poll` updates.
    b.schedule(stage::INGEST_LOCAL, "appearance.poll", |s, _, _| {
        crate::appearance::poll(s)
    })
    .after("egg.poll");
    b.schedule(stage::INGEST_LOCAL, "text_rendering.poll", |s, _, tx| {
        crate::text_rendering::poll(s, tx);
        false
    });
    // Precaution: `updater.poll` reads `I18N` via `t!("common.requestFailed")` before locale changes.
    b.schedule(stage::INGEST_LOCAL, "language.poll", |s, _, tx| {
        crate::language::poll(s, tx)
    })
    .after("updater.poll");

    // Precaution: `notifications.poll` can change `route` or answer `pending`/`requests` read by `speech.poll`.
    b.schedule(stage::INGEST_DISPATCH, "speech.poll", |s, _, tx| {
        crate::speech::poll(s, tx)
    })
    .after("notifications.poll");
    // A shortcut from the composer that moved the keyboard into the side region (its command
    // may run a frame later) leaves the selected tab's label muted (`FocusMode::Quiet`).
    // Known: `egg.poll` (in `INGEST_LOCAL`) and `notifications.poll` can focus the composer;
    // precaution: `speech.poll` adjusts focus before composer focus sync.
    b.schedule(stage::INGEST_DISPATCH, "panels.sync_composer", |s, _, _| {
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
    })
    .after("speech.poll");
    // Known: requires `launch.poll` (`INGEST_SERVERS`) for `Source::Live { writes: Some(_) }`;
    // precaution: mutates `settings.open`, `route`, `tabs`, `projects`, and `composer.focused`.
    b.schedule(stage::INGEST_DISPATCH, "deep_links.drain", |s, _, tx| {
        crate::deep_links::drain(s, tx)
    })
    .after("panels.sync_composer");

    b.schedule(stage::INGEST_SESSION_PRE, "turn_errors.poll", |s, _, tx| {
        crate::turn_errors::poll(s, tx)
    });

    b.schedule(stage::INGEST_PAGES, "session_pages.poll", |s, _, tx| {
        s.poll_pages(tx)
    });
    // Precaution: both `session_pages.poll` and `app.output_errors` push toasts via `dialogs.toast`.
    b.schedule(stage::INGEST_PAGES, "app.output_errors", |s, _, _| {
        let mut changed = false;
        while let Some(why) = s
            .kernel
            .slots
            .get::<crate::app::OutputErrors>()
            .and_then(|e| e.0.as_ref())
            .and_then(|(_, rx)| rx.try_recv().ok())
        {
            crate::status_toast::fail(s, opencode_i18n::t!("common.requestFailed"), why);
            changed = true;
        }
        changed
    })
    .after("session_pages.poll");
    // Known: `session_pages.poll` updates `pages.first_read_done` and calls `context_window_landed`.
    b.schedule(stage::INGEST_PAGES, "context_window.poll", |s, _, _| {
        s.poll_context_window()
    })
    .after("session_pages.poll");
    // Precaution: shares `dialogs.toast` with `app.output_errors`.
    b.schedule(stage::INGEST_PAGES, "inbox.sends", |s, _, _| {
        s.poll_sends()
    })
    .after("app.output_errors");
    // Known: `inbox.sends` (`take_mail`/receipts) unblocks `pump_outbox`.
    b.schedule(stage::INGEST_PAGES, "inbox.outbox", |s, _, tx| {
        s.tick_outbox(tx)
    })
    .after("inbox.sends");
    // Precaution: `jobs.poll` executes arbitrary `Apply` closures that can touch any `App` state.
    b.schedule(stage::INGEST_PAGES, "jobs.poll", |s, _, tx| s.poll_jobs(tx))
        .after_all(&["context_window.poll", "inbox.outbox", "open_in.poll"]);

    b.schedule(stage::INGEST_STREAM, "tick.stream", crate::tick::poll_stream);
    // Known: `tick.submit` commits staged reverts and mutates `composer`/`inboxes`/`transcripts`/`route`.
    b.schedule(stage::INGEST_STREAM, "tick.submit", |s, _, tx| {
        crate::tick::poll_submit(s, tx)
    })
    .after("history.jump");

    // ── Phase 2: Derive ──────────────────────────────────────────────────────────────────────────
    b.schedule(stage::DERIVE_TIMELINE, "working.sync", |s, _, _| {
        crate::working::sync(s)
    });
    b.schedule(stage::DERIVE_TIMELINE, "timeline.dock_morph", |s, _, tx| {
        let asked = s.take_asked_dock_face();
        if s.dock_face() != asked {
            s.morph(tx);
        }
        false
    });
    // Known: `subagents.shells::followed` reads `expanded` before `disclosure.poll` drops closed folds.
    b.schedule(stage::DERIVE_TIMELINE, "disclosure.poll", crate::disclosure::poll)
        .after("subagents.shells");
    // Known: `projection.rs` reads `expanded` updated by `disclosure.poll`.
    b.schedule(stage::DERIVE_TIMELINE, "timeline.projection", |s, _, tx| {
        session_view::timeline::projection_poll(s, tx)
    })
    .after("disclosure.poll");
    b.schedule(stage::DERIVE_TIMELINE, "line_reveal.poll", |s, now, tx| {
        crate::line_reveal::poll(s, now, tx)
    });
    b.schedule(stage::DERIVE_TIMELINE, "copied.poll", crate::copied::poll);
    b.schedule(stage::DERIVE_TIMELINE, "diagram.poll", crate::diagram::poll);
    b.schedule(stage::DERIVE_TIMELINE, "timeline.images", |s, now, tx| {
        session_view::images::poll(s, now, tx)
    });
    b.schedule(stage::DERIVE_TIMELINE, "model_picker.hover", |s, now, tx| {
        crate::model_picker::hover::poll(s, now, tx)
    });
    b.schedule(stage::DERIVE_TIMELINE, "background.poll", |s, _, tx| {
        crate::background::poll(s, tx)
    });

    // Ingest local answers before settling the list: its rows, highlight and empty state must
    // all see the same sources. Previously a full-text landing changed the rows after this poll.
    // Known: `poll_fulltext` reads `home_search` updated by `workspace_tools.home_search`.
    b.schedule(stage::DERIVE_SEARCH, "switcher.poll", |s, now, tx| {
        let local_answers = s.poll_session_search(now, tx) | s.poll_fulltext();
        s.poll_switcher(tx, local_answers)
    })
    .after("workspace_tools.home_search");

    b.schedule(stage::DERIVE_COMPOSER, "mentions.poll", |s, _, tx| {
        s.poll_mentions(tx)
    });

    // ── Phase 3: React ───────────────────────────────────────────────────────────────────────────
    b.schedule(stage::REACT_PANELS, "composer_input.attach", |s, _, _| {
        if s.composer_ext.attach.poll() {
            rune::Tracked::touch(&s.composer_ext.attach);
            true
        } else {
            false
        }
    });
    b.schedule(stage::REACT_PANELS, "panels.poll", |s, _, tx| {
        s.poll_panels(tx)
    });
    // Precaution: `panels.poll` (`review`, plugins) and `workspace_tools.rune_mcp` (`tools::execute`)
    // can mutate `source`/session state before turn diffs start.
    b.schedule(stage::REACT_PANELS, "turn_diffs.poll", |s, _, _| {
        s.poll_turn_diffs()
    })
    .after("workspace_tools.rune_mcp");

    b.schedule(stage::REACT_STATUS, "reconnect.poll", |s, _, tx| {
        crate::reconnect::poll(s, tx)
    });
    // Known: `reconnect.poll` calls `session_state::reconnected` mutating `services.session_states`.
    b.schedule(stage::REACT_STATUS, "session_state.poll", |s, _, tx| {
        crate::session_state::poll(s, tx)
    })
    .after("reconnect.poll");
    // Known: `tips.poll` can push a toast polled here, and closing an undo-delete toast calls
    // `undo::toast_closed` -> `send_delete` before `REACT_SHELL`.
    b.schedule(stage::REACT_STATUS, "status_toast.poll", |s, now, tx| {
        crate::status_toast::poll(s, now, tx)
    })
    .after("tips.poll");

    b.schedule(stage::REACT_SHELL, "layout_state.poll", |s, now, tx| {
        crate::layout_state::poll(s, now, tx)
    });
    // Known/precaution: reads `tab_layout()` and `narrow` settled by `layout_state.poll` -> `narrow.poll`.
    b.schedule(stage::REACT_SHELL, "inbox_model.poll", |s, now, tx| {
        crate::inbox_model::poll(s, now, tx)
    })
    .after("narrow.poll");
    // Known: reads `narrow()` and `tab_layout()` settled by `layout_state.poll` -> `narrow.poll`.
    b.schedule(stage::REACT_SHELL, "tab_preview.poll", |s, now, tx| {
        crate::view::tab_preview::poll(s, now, tx)
    })
    .after("narrow.poll");
    // Known: reads `tab_layout()` and `settings.open` settled by `layout_state.poll` -> `narrow.poll`.
    b.schedule(stage::REACT_SHELL, "session_status.poll", |s, now, tx| {
        crate::session_status::poll(s, now, tx)
    })
    .after("narrow.poll");
    // Each draft keeps its own composer in any window, saved or not (`restore::show_draft`).
    // Known: runs after `status_toast.poll` (`REACT_STATUS`) and `narrow.poll` settle `route`,
    // and after `composer_input.attach` (`REACT_PANELS`).
    b.schedule(stage::REACT_SHELL, "restore.show_draft", |s, _, _| {
        s.show_draft()
    })
    .after("narrow.poll");

    // ── Phase 4: Persist ─────────────────────────────────────────────────────────────────────────
    b.schedule(stage::PERSIST_FLUSH, "restore.persist", |s, _, tx| {
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
    b.schedule(stage::PERSIST_FLUSH, "annotations.poll_saved", |s, _, _| {
        s.persists() && crate::line_comments::poll_saved(s)
    });
    // Known: `restore.persist` runs `hydrate_restore` mutating `tabs`/`route`/`window` before
    // window, taskbar, and jump-list persistence.
    b.schedule(stage::PERSIST_FLUSH, "windows.persist", |s, _, _| {
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
    })
    .after("restore.persist");
}
