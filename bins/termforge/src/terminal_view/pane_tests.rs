//! Drives `TerminalView` headlessly (GPUI's test platform, real shells) to
//! exercise split panes end to end: keyboard, mouse and session routing.

use std::time::{Duration, Instant};

use gpui::{
    point, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta,
    ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext,
};

use super::*;

type View = Entity<TerminalView>;

/// Keep the test away from any running daemon and the user's data.
fn isolate() {
    let dir = std::env::temp_dir().join(format!("tfp{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for var in ["XDG_RUNTIME_DIR", "XDG_DATA_HOME", "XDG_CONFIG_HOME"] {
        std::env::set_var(var, &dir);
    }
    std::env::set_var("TERMFORGE_DATA_DIR", dir.join("data"));
}

fn count(view: &View, cx: &mut VisualTestContext) -> usize {
    view.read_with(cx, |v, _| v.tabs[v.active_tab].layout.len())
}

fn focused(view: &View, cx: &mut VisualTestContext) -> PaneId {
    view.read_with(cx, |v, _| v.tabs[v.active_tab].layout.focused())
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

/// Poll until `check` passes; shells run on real threads.
fn wait_for(
    cx: &mut VisualTestContext,
    what: &str,
    mut check: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn open(cx: &mut TestAppContext) -> (View, &mut VisualTestContext) {
    isolate();
    let spawn = crate::spawn_options().expect("a shell is installed");
    let (view, cx) = cx.add_window_view(|window, cx| {
        TerminalView::new(
            spawn,
            None,
            crate::appearance::Appearance::builtin(),
            window,
            cx,
        )
    });
    redraw(cx);
    (view, cx)
}

fn open_file(view: &View, cx: &mut VisualTestContext, name: &str) {
    let path = PathBuf::from(name);
    view.update(cx, |view, cx| view.open_project_file(path.clone(), cx));
    wait_for(cx, "editor file load", |cx| {
        view.read_with(cx, |view, _| {
            !view.editor_loading && view.selected_project_file.as_ref() == Some(&path)
        })
    });
}

#[gpui::test]
async fn workbench_starts_project_first_and_terminal_can_maximize(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.read_with(cx, |view, _| {
        assert!(
            view.show_tab_strip,
            "terminal tabs remain visible in their own header strip"
        );
        assert_eq!(view.workspace_artifact, WorkspaceArtifact::Overview);
        assert_eq!(view.dock_tool, DockTool::Terminal);
        assert!(!view.dock_collapsed);
        assert!(!view.terminal_maximized, "project overview is visible");
    });

    view.update(cx, |view, cx| view.toggle_terminal_maximized(cx));
    redraw(cx);
    assert!(view.read_with(cx, |view, _| view.terminal_maximized));

    view.update(cx, |view, cx| view.toggle_terminal_maximized(cx));
    redraw(cx);
    assert!(!view.read_with(cx, |view, _| view.terminal_maximized));

    view.update(cx, |view, cx| view.toggle_dock_collapsed(cx));
    redraw(cx);
    assert!(view.read_with(cx, |view, _| view.dock_collapsed));
}

#[gpui::test]
async fn split_focus_and_close_by_keyboard(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let first = focused(&view, cx);
    assert_eq!(count(&view, cx), 1);

    cx.simulate_keystrokes("ctrl-shift-d");
    redraw(cx);
    assert_eq!(count(&view, cx), 2, "ctrl-shift-d splits");
    let second = focused(&view, cx);
    assert_ne!(first, second, "the new pane takes focus");

    cx.simulate_keystrokes("ctrl-alt-left");
    redraw(cx);
    assert_eq!(
        focused(&view, cx),
        first,
        "ctrl-alt-left focuses the pane on the left"
    );
    cx.simulate_keystrokes("ctrl-alt-right");
    redraw(cx);
    assert_eq!(focused(&view, cx), second);

    cx.simulate_keystrokes("ctrl-shift-e");
    redraw(cx);
    assert_eq!(count(&view, cx), 3, "ctrl-shift-e splits down");

    cx.simulate_keystrokes("ctrl-shift-x ctrl-shift-x");
    redraw(cx);
    assert_eq!(count(&view, cx), 1, "ctrl-shift-x closes panes");
    assert_eq!(focused(&view, cx), first);
    // Closing the last pane would close the workspace; the layout is intact.
    assert_eq!(view.read_with(cx, |v, _| v.tabs.len()), 1);
}

#[gpui::test]
async fn pane_focus_swaps_sessions_and_close_promotes_the_sibling(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let first = focused(&view, cx);
    let first_session = view.read_with(cx, |v, _| {
        v.session
            .as_ref()
            .cloned()
            .expect("initial pane has a session handle")
    });

    view.update(cx, |v, cx| v.split_pane(Axis::Horizontal, cx));
    let second = focused(&view, cx);
    let second_session = view.read_with(cx, |v, _| {
        v.session
            .as_ref()
            .cloned()
            .expect("new pane has a session handle")
    });
    assert!(!Arc::ptr_eq(&first_session, &second_session));

    view.update(cx, |v, cx| v.focus_pane(first, cx));
    view.read_with(cx, |v, _| {
        assert_eq!(v.tabs[v.active_tab].layout.focused(), first);
        assert!(Arc::ptr_eq(v.session.as_ref().unwrap(), &first_session));
        assert!(Arc::ptr_eq(
            v.tabs[v.active_tab].panes[&second]
                .session
                .as_ref()
                .unwrap(),
            &second_session,
        ));
    });

    // Closing the focused pane promotes the exact parked sibling; it does
    // not create a replacement shell or leave the closed pane in the tree.
    view.update(cx, |v, cx| v.close_pane(cx));
    view.read_with(cx, |v, _| {
        let tab = &v.tabs[v.active_tab];
        assert_eq!(tab.layout.ids(), vec![second]);
        assert!(tab.panes.is_empty());
        assert!(Arc::ptr_eq(v.session.as_ref().unwrap(), &second_session));
    });
}

#[gpui::test]
async fn dragging_a_divider_changes_the_ratio(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.simulate_keystrokes("ctrl-shift-d");
    redraw(cx);
    redraw(cx);

    let area = view.read_with(cx, |v, _| v.pane_area);
    assert!(
        area.size.width > px(100.0),
        "pane area was never recorded: {area:?}"
    );
    let ratio = |view: &View, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            v.tabs[v.active_tab]
                .layout
                .dividers(PaneRect::new(0.0, 0.0, 1.0, 1.0))[0]
                .ratio
        })
    };
    assert!((ratio(&view, cx) - 0.5).abs() < 1e-3);

    let y = area.origin.y + area.size.height / 2.0;
    let from = point(area.origin.x + area.size.width * 0.5, y);
    let to = point(area.origin.x + area.size.width * 0.25, y);
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: from,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseMoveEvent {
        position: to,
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: to,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    redraw(cx);
    let after = ratio(&view, cx);
    assert!((after - 0.25).abs() < 0.03, "ratio after drag: {after}");

    // Dragging far past the edge clamps instead of collapsing a pane.
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: point(area.origin.x + area.size.width * after, y),
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseMoveEvent {
        position: point(area.origin.x - px(500.0), y),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: point(area.origin.x - px(500.0), y),
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    redraw(cx);
    assert!(
        (ratio(&view, cx) - 0.1).abs() < 1e-3,
        "clamped to the minimum"
    );
}

#[gpui::test]
async fn hidden_sidebar_opens_when_the_left_edge_is_hovered(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| view.toggle_sidebar(cx));
    redraw(cx);
    assert!(!view.read_with(cx, |view, _| view.show_sidebar));

    cx.simulate_event(MouseMoveEvent {
        position: point(px(2.0), px(100.0)),
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    redraw(cx);
    assert!(
        view.read_with(cx, |view, _| view.sidebar_peek),
        "hovering the left-edge hit zone opens the sidebar"
    );
}

#[gpui::test]
async fn wheel_over_a_parked_pane_scrolls_that_pane(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let first = focused(&view, cx);
    // Fill the first pane's scrollback, then split so it is parked.
    view.read_with(cx, |v, _| {
        // `echo` is accepted by PowerShell, cmd.exe and Unix shells. Sending
        // separate commands also exercises the real interactive input path.
        let commands = "echo tf-pane-test\r".repeat(400);
        v.session
            .as_ref()
            .unwrap()
            .write(commands.as_bytes())
            .unwrap();
    });
    wait_for(cx, "scrollback in the first pane", |cx| {
        view.read_with(cx, |v, _| {
            v.session
                .as_ref()
                .unwrap()
                .with(|p| p.engine().history_size() > 100)
        })
    });
    cx.simulate_keystrokes("ctrl-shift-d");
    redraw(cx);
    redraw(cx);
    assert_ne!(focused(&view, cx), first);

    let offset = |view: &View, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            v.tabs[v.active_tab].panes[&first]
                .session
                .as_ref()
                .unwrap()
                .with(|p| p.engine().display_offset())
        })
    };
    let focused_offset = |view: &View, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            v.session
                .as_ref()
                .unwrap()
                .with(|p| p.engine().display_offset())
        })
    };
    assert_eq!(offset(&view, cx), 0);

    // The parked pane is on the left half of the pane area.
    let area = view.read_with(cx, |v, _| v.pane_area);
    let over_parked = point(
        area.origin.x + area.size.width * 0.25,
        area.origin.y + area.size.height * 0.5,
    );
    cx.simulate_event(ScrollWheelEvent {
        position: over_parked,
        delta: ScrollDelta::Lines(point(0.0, 5.0)),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    redraw(cx);
    assert!(offset(&view, cx) > 0, "the pane under the pointer scrolled");
    assert_eq!(focused_offset(&view, cx), 0, "the focused pane did not");
}

#[gpui::test]
async fn wheel_in_settings_does_not_scroll_the_terminal(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.read_with(cx, |v, _| {
        let commands = "echo tf-settings-scroll-test\r".repeat(400);
        v.session
            .as_ref()
            .unwrap()
            .write(commands.as_bytes())
            .unwrap();
    });
    wait_for(cx, "terminal scrollback", |cx| {
        view.read_with(cx, |v, _| {
            v.session
                .as_ref()
                .unwrap()
                .with(|p| p.engine().history_size() > 100)
        })
    });
    view.update(cx, |v, cx| {
        v.settings = Some(SettingsPage::Advanced);
        cx.notify();
    });
    redraw(cx);

    let offset = |cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            v.session
                .as_ref()
                .unwrap()
                .with(|p| p.engine().display_offset())
        })
    };
    assert_eq!(offset(cx), 0);
    let area = view.read_with(cx, |v, _| v.pane_area);
    cx.simulate_event(ScrollWheelEvent {
        position: point(
            area.origin.x + area.size.width * 0.5,
            area.origin.y + area.size.height * 0.5,
        ),
        delta: ScrollDelta::Lines(point(0.0, 5.0)),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    redraw(cx);

    assert_eq!(
        offset(cx),
        0,
        "settings wheel leaked into terminal scrollback"
    );
}

#[gpui::test]
async fn theme_picker_previews_then_reverts_or_commits(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    assert!(view.read_with(cx, |v, _| v.theme.is_dark));

    // Typing narrows to "Light" and previews it without saving.
    view.update(cx, |v, cx| v.open_picker(PickerMode::Themes, cx));
    cx.simulate_keystrokes("l i g h t");
    redraw(cx);
    assert!(
        !view.read_with(cx, |v, _| v.theme.is_dark),
        "preview applied"
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.appearance.config.ui.theme_name.clone()),
        "",
        "a preview is not saved"
    );

    // Escape throws the preview away.
    cx.simulate_keystrokes("escape");
    redraw(cx);
    assert!(
        view.read_with(cx, |v, _| v.theme.is_dark),
        "preview reverted"
    );
    assert!(view.read_with(cx, |v, _| v.palette.is_none()));

    // Enter commits.
    view.update(cx, |v, cx| v.open_picker(PickerMode::Themes, cx));
    cx.simulate_keystrokes("l i g h t enter");
    redraw(cx);
    view.read_with(cx, |v, _| {
        assert!(!v.theme.is_dark);
        assert_eq!(v.appearance.config.ui.theme_name, "Light");
        assert!(v.palette.is_none());
    });
}

#[gpui::test]
async fn appearance_settings_update_the_live_view(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |v, cx| {
        v.set_line_height(1.55, cx);
        v.set_cursor_style(CursorStyleConfig::Bar, cx);
        v.set_ligatures(true, cx);
        v.set_accent("#ff8800", cx);
    });
    view.read_with(cx, |v, _| {
        assert!((v.line_height - 1.55).abs() < 1e-6);
        assert_eq!(v.cursor_style, CursorStyleConfig::Bar);
        assert_eq!(v.theme.accent, Rgb::new(0xff, 0x88, 0x00));
    });
}

#[gpui::test]
async fn changes_dock_lists_files_and_opens_a_diff(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let repo = std::env::temp_dir().join(format!("tfrepo{}", std::process::id()));
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "init",
    ]);
    std::fs::write(repo.join("a.txt"), "one\nhello\n").unwrap();

    view.update(cx, |v, cx| {
        let active = v.active_tab;
        v.tabs[active].cwd = Some(repo.to_string_lossy().into_owned());
        v.cwd = v.tabs[active].cwd.clone();
        v.toggle_utility_dock(cx);
        v.set_dock_tab(DockTab::Changes, cx);
    });
    redraw(cx);
    let change = view.read_with(cx, |v, _| {
        v.changes.as_ref().and_then(|c| c.first().cloned())
    });
    let change = change.expect("a.txt is modified");
    assert_eq!(change.path, "a.txt");

    view.update(cx, |v, cx| v.open_diff(&change, cx));
    wait_for(cx, "working-tree diff", |cx| {
        view.read_with(cx, |view, _| {
            !view.git_diff_loading && view.diff_lines.iter().any(|line| line == "+hello")
        })
    });
    view.read_with(cx, |v, _| {
        assert_eq!(v.workspace_artifact, WorkspaceArtifact::Changes);
        assert_eq!(v.inspector, Inspector::Diff("a.txt".into()));
        assert!(
            v.diff_lines.iter().any(|l| l == "+hello"),
            "{:?}",
            v.diff_lines
        );
    });
    let _ = std::fs::remove_dir_all(&repo);
}

#[gpui::test]
async fn git_diff_results_follow_project_and_latest_selection(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    for (root, content) in [(&first, "first project"), (&second, "second project")] {
        std::fs::create_dir(root).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());
        assert!(git(&["config", "user.name", "TermForge Test"])
            .status
            .success());
        assert!(git(&["config", "user.email", "test@termforge.local"])
            .status
            .success());
        std::fs::write(root.join("same.txt"), "base\n").unwrap();
        assert!(git(&["add", "same.txt"]).status.success());
        assert!(git(&["commit", "-qm", "initial"]).status.success());
        std::fs::write(root.join("same.txt"), format!("{content}\n")).unwrap();
    }
    let first = dunce::canonicalize(first).unwrap();
    let second = dunce::canonicalize(second).unwrap();
    let first_change = surface::git_changes(&first).unwrap().remove(0);
    let second_change = surface::git_changes(&second).unwrap().remove(0);
    view.update(cx, |view, cx| {
        view.switch_project(first, cx);
        view.open_workbench_diff(first_change, surface::DiffScope::Unstaged, cx);
        view.switch_project(second, cx);
        view.open_workbench_diff(second_change, surface::DiffScope::Unstaged, cx);
    });
    wait_for(cx, "latest project diff", |cx| {
        view.read_with(cx, |view, _| {
            !view.git_diff_loading && view.diff_lines.iter().any(|line| line == "+second project")
        })
    });
    view.read_with(cx, |view, _| {
        assert!(view.diff_lines.iter().all(|line| line != "+first project"));
        assert!(view.git_diff_error.is_none());
    });
}

#[gpui::test]
async fn dock_edge_resizes_by_dragging(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |v, cx| {
        v.toggle_utility_dock(cx);
        v.edge_drag = Some(EdgeDrag {
            pointer: 500.0,
            size: v.dock_height,
        });
    });
    redraw(cx);
    cx.simulate_event(MouseMoveEvent {
        position: point(px(300.0), px(400.0)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    redraw(cx);
    assert_eq!(view.read_with(cx, |v, _| v.dock_height), 268.0);
}

#[gpui::test]
async fn dock_watches_another_session_without_a_new_process(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |v, cx| {
        v.new_local_tab(cx);
        v.select_tab(0, cx);
    });
    let (sessions_before, other) = view.read_with(cx, |v, _| {
        let ids: Vec<_> = v
            .tabs
            .iter()
            .filter_map(|t| t.session.as_ref().map(|s| s.id()))
            .collect();
        (ids, v.tabs[1].id)
    });
    view.update(cx, |v, cx| v.watch_in_dock(other, cx));
    redraw(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.dock_tab, DockTab::Output);
        let after: Vec<_> = v
            .tabs
            .iter()
            .filter_map(|t| t.session.as_ref().map(|s| s.id()))
            .collect();
        assert_eq!(after, sessions_before, "watching must not spawn sessions");
    });
}

#[gpui::test]
async fn dock_state_round_trips_and_is_clamped(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |v, cx| {
        v.toggle_utility_dock(cx);
        v.dock_height = 300.0;
        v.dock_tab = DockTab::Output;
        let saved = v.saved_chrome();
        assert_eq!(saved.dock_tab, "output");
        assert!(saved.docked_session.is_none());

        v.show_utility_dock = false;
        v.dock_tab = DockTab::Sessions;
        v.apply_saved_chrome(&saved);
        assert!(v.show_utility_dock);
        assert_eq!(v.dock_height, 300.0);
        // Output needs a watched session, which a restart loses.
        assert_eq!(v.dock_tab, DockTab::Sessions);

        let hostile = SavedChrome {
            dock_open: true,
            dock_height: 99_999,
            dock_tab: "bogus".into(),
            panel_open: true,
            panel_width: 1,
            docked_session: None,
        };
        v.apply_saved_chrome(&hostile);
        assert_eq!(v.dock_height, 520.0);
    });
}

#[gpui::test]
async fn managed_process_state_is_observed_from_the_shell(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |v, cx| {
        v.start_managed("echoer".into(), "echo tf-managed".into(), false, cx)
    });
    let index = view.read_with(cx, |v, _| v.active_tab);
    view.read_with(cx, |v, _| {
        let tab = &v.tabs[index];
        assert_eq!(tab.name.as_deref(), Some("echoer"));
        assert!(tab.managed.is_some());
        assert!(!tab.managed.as_ref().unwrap().agent);
    });
    wait_for(cx, "the command to be observed finishing", |cx| {
        view.read_with(cx, |v, _| {
            TerminalView::managed_state(&v.tabs[index]) == Some(surface::ProcessState::Succeeded)
        })
    });
    // A plain terminal is never reported as managed.
    view.read_with(cx, |v, _| {
        assert!(TerminalView::managed_state(&v.tabs[0]).is_none());
    });
}

#[gpui::test]
async fn docked_session_takes_input_and_moves_back(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |v, cx| v.new_local_tab(cx));
    let (docked_id, docked_index) = view.read_with(cx, |v, _| (v.tabs[1].id, 1));
    // Dock the workspace on the canvas: another one must take its place.
    view.update(cx, |v, cx| v.move_to_dock(docked_id, cx));
    redraw(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.active_tab, 0, "the canvas switched to another workspace");
        assert!(v.dock_moved && v.dock_focus && v.show_utility_dock);
        let docked = v.docked().expect("docked session resolves");
        assert_eq!(docked.0, docked_index);
        assert_eq!(
            v.saved_chrome().docked_session,
            v.tabs[docked_index]
                .session
                .as_ref()
                .and_then(|session| session.id())
                .map(|id| id.to_string()),
            "the durable layout references the same daemon session"
        );
    });

    view.read_with(cx, |v, _| v.write(b"echo tf-docked\r"));
    wait_for(cx, "the command to run in the docked session", |cx| {
        view.read_with(cx, |v, _| {
            v.tabs[docked_index]
                .session
                .as_ref()
                .is_some_and(|s| s.with(|p| p.blocks().finished()) >= 1)
        })
    });
    view.read_with(cx, |v, _| {
        let main = v.tabs[0].session.as_ref().unwrap();
        assert_eq!(
            main.with(|p| p.blocks().finished()),
            0,
            "the canvas session did not receive the keystrokes"
        );
    });

    // Selecting it from anywhere moves it back, still the same session.
    view.update(cx, |v, cx| v.select_tab(docked_index, cx));
    view.read_with(cx, |v, _| {
        assert!(!v.dock_moved && !v.dock_focus && v.dock_target.is_none());
        assert_eq!(v.active_tab, docked_index);
        assert_eq!(v.tabs.len(), 2, "no session was created or closed");
        assert!(v.saved_chrome().docked_session.is_none());
    });
}

#[gpui::test]
async fn switching_project_resets_per_project_state(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = std::env::temp_dir().join(format!("tfproj{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = dunce::canonicalize(&root).unwrap();
    view.update(cx, |v, cx| {
        v.new_local_tab(cx);
        v.dock_target = Some(v.tabs[1].id);
        v.dock_moved = true;
        v.tasks.items.push(tf_proto::TaskInfo {
            id: tf_proto::TaskId::default(),
            project_root: PathBuf::from("/previous-project"),
            title: "Previous project task".into(),
            context: String::new(),
            command: None,
            state: tf_proto::TaskState::Active,
            priority: tf_proto::TaskPriority::Normal,
            dependencies: Vec::new(),
            updated_at: 0,
        });
        v.switch_project(root.clone(), cx);
        assert!(
            v.tasks.items.is_empty(),
            "old project tasks must not appear during loading"
        );
    });
    redraw(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.local_project_root.as_ref(), Some(&root));
        assert_eq!(v.tabs.len(), 1, "the new project starts with one workspace");
        assert!(v.dock_target.is_none() && !v.dock_moved);
        assert_eq!(
            v.projects.first(),
            Some(&root),
            "current project listed first"
        );
        assert!(v.session.is_some());
    });
    // Switching to the project already open is a no-op.
    view.update(cx, |v, cx| v.switch_project(root.clone(), cx));
    assert_eq!(view.read_with(cx, |v, _| v.tabs.len()), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui::test]
async fn files_search_navigates_buffer_matches(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = std::env::temp_dir().join(format!("tfeditor{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("notes.txt"), "alpha beta alpha").unwrap();
    let root = dunce::canonicalize(&root).unwrap();

    view.update(cx, |view, cx| {
        view.switch_project(root.clone(), cx);
        view.workspace_artifact = WorkspaceArtifact::Files;
    });
    open_file(&view, cx, "notes.txt");
    view.update(cx, |view, _| {
        view.editor_search = Some(EditorSearchState {
            query: "alpha".into(),
            ..EditorSearchState::default()
        });
        view.recompute_editor_search();
    });
    view.read_with(cx, |view, _| {
        let search = view.editor_search.as_ref().unwrap();
        assert_eq!(search.matches, vec![0..5, 11..16]);
        assert_eq!(view.editor_buffer.as_ref().unwrap().selection(), 0..5);
    });

    view.update(cx, |view, cx| view.move_editor_search(false, cx));
    assert_eq!(
        view.read_with(cx, |view, _| view
            .editor_buffer
            .as_ref()
            .unwrap()
            .selection()),
        11..16
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui::test]
async fn files_keyboard_navigates_selects_and_deletes(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("file.txt"), "first\n🙂abc\nlast").unwrap();
    let project = dunce::canonicalize(root.path()).unwrap();
    view.update(cx, |view, cx| {
        view.switch_project(project, cx);
        view.workspace_artifact = WorkspaceArtifact::Files;
    });
    open_file(&view, cx, "file.txt");
    redraw(cx);
    cx.simulate_keystrokes("down end shift-left shift-left delete");
    view.read_with(cx, |view, _| {
        let buffer = view.editor_buffer.as_ref().unwrap();
        assert_eq!(buffer.text(), "first\n🙂a\nlast");
        assert!(buffer.is_dirty());
    });
    cx.simulate_keystrokes("ctrl-home down home delete");
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.editor_buffer.as_ref().unwrap().text(),
            "first\na\nlast"
        );
    });
}

#[gpui::test]
async fn files_clipboard_shortcuts_edit_the_buffer(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("file.txt"), "hello 🙂").unwrap();
    let project = dunce::canonicalize(root.path()).unwrap();
    view.update(cx, |view, cx| {
        view.switch_project(project, cx);
        view.workspace_artifact = WorkspaceArtifact::Files;
    });
    open_file(&view, cx, "file.txt");
    redraw(cx);
    cx.simulate_keystrokes("ctrl-a ctrl-c ctrl-end ctrl-v");
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("hello 🙂"));
    assert_eq!(view.read_with(cx, |view, _| view.editor_buffer.as_ref().unwrap().text().to_owned()), "hello 🙂hello 🙂");
    cx.simulate_keystrokes("ctrl-a ctrl-x");
    assert_eq!(view.read_with(cx, |view, _| view.editor_buffer.as_ref().unwrap().text().to_owned()), "");
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(view.read_with(cx, |view, _| view.editor_buffer.as_ref().unwrap().text().to_owned()), "hello 🙂hello 🙂");
}

#[gpui::test]
async fn editor_open_ignores_superseded_results(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(first.join("old.txt"), "old project").unwrap();
    std::fs::write(second.join("new.txt"), "new project").unwrap();
    let first = dunce::canonicalize(first).unwrap();
    let second = dunce::canonicalize(second).unwrap();
    view.update(cx, |view, cx| {
        view.switch_project(first, cx);
        view.open_project_file(PathBuf::from("missing.txt"), cx);
        view.open_project_file(PathBuf::from("old.txt"), cx);
        view.switch_project(second, cx);
        view.open_project_file(PathBuf::from("new.txt"), cx);
    });
    wait_for(cx, "latest editor file", |cx| {
        view.read_with(cx, |view, _| {
            !view.editor_loading && view.editor_buffer.is_some()
        })
    });
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.selected_project_file.as_deref(),
            Some(Path::new("new.txt"))
        );
        assert_eq!(view.editor_buffer.as_ref().unwrap().text(), "new project");
        assert!(view.editor_error.is_none());
    });
}

#[gpui::test]
async fn diagnostics_replace_per_file_and_open_unicode_position(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "one").unwrap();
    std::fs::write(root.path().join("two.txt"), "🙂target").unwrap();
    let project = dunce::canonicalize(root.path()).unwrap();
    view.update(cx, |view, cx| view.switch_project(project.clone(), cx));
    open_file(&view, cx, "one.txt");
    let first = crate::lsp::Diagnostic {
        path: project.join("one.txt"),
        start: tf_editor::TextPosition {
            line: 0,
            utf16_column: 0,
        },
        end: tf_editor::TextPosition {
            line: 0,
            utf16_column: 1,
        },
        severity: crate::lsp::Severity::Warning,
        source: None,
        message: "first".into(),
    };
    let second = crate::lsp::Diagnostic {
        path: project.join("two.txt"),
        start: tf_editor::TextPosition {
            line: 0,
            utf16_column: 2,
        },
        end: tf_editor::TextPosition {
            line: 0,
            utf16_column: 3,
        },
        severity: crate::lsp::Severity::Error,
        source: None,
        message: "second".into(),
    };
    view.update(cx, |view, cx| {
        view.update_file_diagnostics(first.path.clone(), Some(0), vec![first.clone()]);
        view.update_file_diagnostics(second.path.clone(), None, vec![second.clone()]);
        assert_eq!(view.lsp_diagnostics.len(), 2);
        view.update_file_diagnostics(first.path.clone(), Some(9), Vec::new());
        assert_eq!(
            view.lsp_diagnostics.len(),
            2,
            "stale version must not clear current diagnostics"
        );
        view.update_file_diagnostics(first.path.clone(), Some(0), Vec::new());
        assert_eq!(view.lsp_diagnostics, vec![second.clone()]);
        view.open_diagnostic(&second, cx);
    });
    wait_for(cx, "diagnostic file location", |cx| {
        view.read_with(cx, |view, _| {
            !view.editor_loading
                && view.selected_project_file.as_deref() == Some(Path::new("two.txt"))
        })
    });
    view.read_with(cx, |view, _| {
        assert_eq!(view.workspace_artifact, WorkspaceArtifact::Files);
        assert_eq!(view.editor_buffer.as_ref().unwrap().selection(), 4..4);
        assert_eq!(view.lsp_diagnostics, vec![second.clone()]);
    });
    let mut outside = second.clone();
    outside.path = project.join("..").join("outside.txt");
    view.update(cx, |view, cx| view.open_diagnostic(&outside, cx));
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_project_file.clone()),
        Some(PathBuf::from("two.txt"))
    );
}

#[gpui::test]
async fn diagnostic_and_search_reveal_offscreen_editor_lines(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = tempfile::tempdir().unwrap();
    let mut text = (0..120)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    text.push_str("search target\n");
    std::fs::write(root.path().join("long.txt"), text).unwrap();
    let project = dunce::canonicalize(root.path()).unwrap();
    view.update(cx, |view, cx| {
        view.switch_project(project.clone(), cx);
        view.workspace_artifact = WorkspaceArtifact::Files;
    });
    open_file(&view, cx, "long.txt");
    redraw(cx);
    let diagnostic = crate::lsp::Diagnostic {
        path: project.join("long.txt"),
        start: tf_editor::TextPosition {
            line: 80,
            utf16_column: 0,
        },
        end: tf_editor::TextPosition {
            line: 80,
            utf16_column: 1,
        },
        severity: crate::lsp::Severity::Warning,
        source: None,
        message: "distant problem".into(),
    };
    view.update(cx, |view, cx| view.open_diagnostic(&diagnostic, cx));
    redraw(cx);
    view.read_with(cx, |view, _| {
        assert!(view.editor_scroll.top_item() <= 80);
        assert!(view.editor_scroll.bottom_item() >= 80);
        assert!(view.editor_scroll.top_item() > 0);
    });
    view.update(cx, |view, cx| {
        view.editor_search = Some(EditorSearchState {
            query: "search target".into(),
            ..EditorSearchState::default()
        });
        view.recompute_editor_search();
        cx.notify();
    });
    redraw(cx);
    view.read_with(cx, |view, _| {
        assert!(view.editor_scroll.top_item() <= 120);
        assert!(view.editor_scroll.bottom_item() >= 120);
    });
}

#[gpui::test]
async fn background_save_preserves_newer_edits_after_switching_files(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "one").unwrap();
    std::fs::write(root.path().join("two.txt"), "two").unwrap();
    let project = dunce::canonicalize(root.path()).unwrap();
    view.update(cx, |view, cx| view.switch_project(project.clone(), cx));
    open_file(&view, cx, "one.txt");
    view.update(cx, |view, cx| {
        let buffer = view.editor_buffer.as_mut().unwrap();
        buffer.set_selection(3..3);
        buffer.replace_selection(" saved");
        view.save_editor_buffer(cx);
        view.editor_buffer
            .as_mut()
            .unwrap()
            .replace_selection(" later");
        view.open_project_file(PathBuf::from("two.txt"), cx);
    });
    wait_for(cx, "background save", |cx| {
        view.read_with(cx, |view, _| view.editor_saves.is_empty())
    });
    assert_eq!(
        std::fs::read_to_string(project.join("one.txt")).unwrap(),
        "one saved"
    );
    open_file(&view, cx, "one.txt");
    view.read_with(cx, |view, _| {
        let buffer = view.editor_buffer.as_ref().unwrap();
        assert_eq!(buffer.text(), "one saved later");
        assert!(buffer.is_dirty() && buffer.can_undo());
        assert!(view.editor_error.is_none());
    });
}

#[gpui::test]
async fn repeated_save_queues_requested_revision_without_saving_later_edits(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "one").unwrap();
    std::fs::write(root.path().join("two.txt"), "two").unwrap();
    let project = dunce::canonicalize(root.path()).unwrap();
    view.update(cx, |view, cx| view.switch_project(project.clone(), cx));
    open_file(&view, cx, "one.txt");
    view.update(cx, |view, cx| {
        let buffer = view.editor_buffer.as_mut().unwrap();
        buffer.set_selection(3..3);
        buffer.replace_selection(" first");
        view.save_editor_buffer(cx);
        view.editor_buffer
            .as_mut()
            .unwrap()
            .replace_selection(" second");
        view.save_editor_buffer(cx);
        view.editor_buffer
            .as_mut()
            .unwrap()
            .replace_selection(" later");
        view.open_project_file(PathBuf::from("two.txt"), cx);
    });
    wait_for(cx, "queued save", |cx| {
        view.read_with(cx, |view, _| {
            view.editor_saves.is_empty() && view.editor_queued_saves.is_empty()
        })
    });
    assert_eq!(
        std::fs::read_to_string(project.join("one.txt")).unwrap(),
        "one first second"
    );
    open_file(&view, cx, "one.txt");
    view.read_with(cx, |view, _| {
        let buffer = view.editor_buffer.as_ref().unwrap();
        assert_eq!(buffer.text(), "one first second later");
        assert!(buffer.is_dirty());
        assert!(view.editor_error.is_none());
    });
}

#[gpui::test]
async fn background_save_reports_conflicts_for_parked_files(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "one").unwrap();
    std::fs::write(root.path().join("two.txt"), "two").unwrap();
    let project = dunce::canonicalize(root.path()).unwrap();
    view.update(cx, |view, cx| view.switch_project(project.clone(), cx));
    open_file(&view, cx, "one.txt");
    std::fs::write(project.join("one.txt"), "external").unwrap();
    view.update(cx, |view, cx| {
        view.editor_buffer
            .as_mut()
            .unwrap()
            .replace_selection("edit ");
        view.save_editor_buffer(cx);
        view.save_editor_buffer(cx);
        view.open_project_file(PathBuf::from("two.txt"), cx);
    });
    wait_for(cx, "conflicted background save", |cx| {
        view.read_with(cx, |view, _| {
            view.editor_saves.is_empty() && view.editor_queued_saves.is_empty()
        })
    });
    open_file(&view, cx, "one.txt");
    view.read_with(cx, |view, _| {
        assert!(view
            .editor_error
            .as_deref()
            .is_some_and(|error| error.contains("changed on disk")));
        assert_eq!(view.editor_buffer.as_ref().unwrap().text(), "edit one");
    });
    assert_eq!(
        std::fs::read_to_string(project.join("one.txt")).unwrap(),
        "external"
    );
}

#[gpui::test]
async fn unsaved_buffers_survive_file_and_project_switches(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let directory = tempfile::tempdir().unwrap();
    let first_root = directory.path().join("first");
    let second_root = directory.path().join("second");
    std::fs::create_dir_all(&first_root).unwrap();
    std::fs::create_dir_all(&second_root).unwrap();
    std::fs::write(first_root.join("one.txt"), "original").unwrap();
    std::fs::write(first_root.join("two.txt"), "other").unwrap();
    let first_root = dunce::canonicalize(first_root).unwrap();
    let second_root = dunce::canonicalize(second_root).unwrap();

    view.update(cx, |view, cx| {
        view.switch_project(first_root.clone(), cx);
    });
    open_file(&view, cx, "one.txt");
    view.update(cx, |view, _| {
        let buffer = view.editor_buffer.as_mut().unwrap();
        buffer.set_selection(8..8);
        buffer.replace_selection(" edited");
    });
    open_file(&view, cx, "two.txt");
    open_file(&view, cx, "one.txt");
    view.update(cx, |view, cx| {
        let buffer = view.editor_buffer.as_ref().unwrap();
        assert_eq!(buffer.text(), "original edited");
        assert_eq!(buffer.selection(), 15..15);
        assert!(buffer.is_dirty() && buffer.can_undo());
        view.switch_project(second_root.clone(), cx);
        view.switch_project(first_root.clone(), cx);
        view.open_project_file(PathBuf::from("one.txt"), cx);
        let buffer = view.editor_buffer.as_mut().unwrap();
        assert_eq!(buffer.text(), "original edited");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "original");
    });
    assert_eq!(
        std::fs::read_to_string(first_root.join("one.txt")).unwrap(),
        "original"
    );
}

#[gpui::test]
async fn native_close_confirms_unsaved_buffers_in_other_files(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "one").unwrap();
    std::fs::write(root.path().join("two.txt"), "two").unwrap();
    let project = dunce::canonicalize(root.path()).unwrap();
    view.update(cx, |view, cx| {
        view.switch_project(project, cx);
    });
    open_file(&view, cx, "one.txt");
    view.update(cx, |view, cx| {
        view.editor_buffer
            .as_mut()
            .unwrap()
            .replace_selection("edited ");
        view.open_project_file(PathBuf::from("two.txt"), cx);
    });
    assert!(!cx.simulate_close());
    let (message, detail) = cx.pending_prompt().expect("close confirmation");
    assert_eq!(message, "Discard unsaved changes?");
    assert!(detail.contains("1 file contains unsaved changes"));
    assert!(
        !cx.simulate_close(),
        "a repeated close cannot bypass the prompt"
    );
    cx.simulate_prompt_answer("Keep editing");
    cx.run_until_parked();
    assert!(!view.read_with(cx, |view, _| view.close_confirmation_pending));
    view.update(cx, |view, cx| {
        view.open_project_file(PathBuf::from("one.txt"), cx)
    });
    assert_eq!(
        view.read_with(cx, |view, _| view
            .editor_buffer
            .as_ref()
            .unwrap()
            .text()
            .to_owned()),
        "edited one"
    );
    view.update(cx, |view, cx| {
        view.workspace_artifact = WorkspaceArtifact::Files;
        cx.notify();
    });
    redraw(cx);
    cx.simulate_keystrokes("ctrl-shift-q");
    assert!(cx.has_pending_prompt(), "the quit shortcut must confirm dirty files");
    cx.simulate_prompt_answer("Keep editing");
    cx.run_until_parked();
    assert!(!cx.simulate_close());
    cx.simulate_prompt_answer("Discard and close");
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(root.path().join("one.txt")).unwrap(),
        "one"
    );
}

#[gpui::test]
async fn project_file_scans_follow_project_switches_and_refresh(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(first.join("first.txt"), "first").unwrap();
    std::fs::write(second.join("second.txt"), "second").unwrap();
    let first = dunce::canonicalize(first).unwrap();
    let second = dunce::canonicalize(second).unwrap();
    view.update(cx, |view, cx| {
        view.switch_project(first.clone(), cx);
        view.switch_project(second.clone(), cx);
    });
    wait_for(cx, "second project's file snapshot", |cx| {
        view.read_with(cx, |view, _| {
            !view.project_files_loading
                && view
                    .project_file_entries
                    .iter()
                    .any(|entry| entry.path == Path::new("second.txt"))
        })
    });
    view.read_with(cx, |view, _| {
        assert!(view
            .project_file_entries
            .iter()
            .all(|entry| entry.path != Path::new("first.txt")));
    });
    std::fs::write(second.join("new.txt"), "new").unwrap();
    view.update(cx, |view, cx| view.refresh_project_files(cx));
    wait_for(cx, "refreshed project file snapshot", |cx| {
        view.read_with(cx, |view, _| {
            !view.project_files_loading
                && view
                    .project_file_entries
                    .iter()
                    .any(|entry| entry.path == Path::new("new.txt"))
        })
    });
}

#[gpui::test]
async fn workbench_state_round_trips_and_rejects_unsafe_files(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let root = std::env::temp_dir().join(format!("tfworkbench{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("notes.txt"), "restored").unwrap();
    let root = dunce::canonicalize(&root).unwrap();

    let saved = view.update(cx, |view, cx| {
        view.switch_project(root.clone(), cx);
        view.workspace_artifact = WorkspaceArtifact::Tasks;
        view.file_explorer_mode = FileExplorerMode::Smart;
        view.open_project_file(PathBuf::from("notes.txt"), cx);
        view.git_diff_view = GitDiffView::Split;
        view.tasks.filter = tasks::TaskFilter::Blocked;
        view.tasks.sort = tasks::TaskSort::Title;
        view.dock_tool = DockTool::Problems;
        view.dock_collapsed = true;
        view.terminal_maximized = false;
        view.sidebar_width = 260.0;
        view.saved_workbench()
    });

    view.update(cx, |view, cx| {
        view.workspace_artifact = WorkspaceArtifact::Overview;
        view.file_explorer_mode = FileExplorerMode::Tree;
        view.selected_project_file = None;
        view.editor_buffer = None;
        view.git_diff_view = GitDiffView::Unified;
        view.tasks.filter = tasks::TaskFilter::Open;
        view.tasks.sort = tasks::TaskSort::Priority;
        view.dock_tool = DockTool::Terminal;
        view.dock_collapsed = false;
        view.terminal_maximized = true;
        view.sidebar_width = 190.0;
        view.apply_saved_workbench(&saved, cx);
    });
    view.read_with(cx, |view, _| {
        assert_eq!(view.workspace_artifact, WorkspaceArtifact::Tasks);
        assert_eq!(view.file_explorer_mode, FileExplorerMode::Smart);
        assert_eq!(
            view.selected_project_file.as_deref(),
            Some(std::path::Path::new("notes.txt"))
        );
        assert_eq!(view.git_diff_view, GitDiffView::Split);
        assert_eq!(view.tasks.filter, tasks::TaskFilter::Blocked);
        assert_eq!(view.tasks.sort, tasks::TaskSort::Title);
        assert_eq!(view.dock_tool, DockTool::Problems);
        assert!(view.dock_collapsed);
        assert!(!view.terminal_maximized);
        assert_eq!(view.sidebar_width, 260.0);
    });

    let unsafe_state = SavedWorkbench {
        environment: "unknown".into(),
        file_mode: "unknown".into(),
        selected_file: Some("../escape.txt".into()),
        git_diff_view: "unknown".into(),
        task_filter: "unknown".into(),
        task_sort: "unknown".into(),
        dock_tool: "unknown".into(),
        dock_collapsed: false,
        terminal_maximized: true,
        sidebar_width: 1,
    };
    view.update(cx, |view, cx| {
        view.selected_project_file = None;
        view.editor_buffer = None;
        view.apply_saved_workbench(&unsafe_state, cx);
    });
    view.read_with(cx, |view, _| {
        assert_eq!(view.workspace_artifact, WorkspaceArtifact::Overview);
        assert_eq!(view.file_explorer_mode, FileExplorerMode::Tree);
        assert_eq!(view.git_diff_view, GitDiffView::Unified);
        assert_eq!(view.tasks.filter, tasks::TaskFilter::Open);
        assert_eq!(view.tasks.sort, tasks::TaskSort::Priority);
        assert_eq!(view.dock_tool, DockTool::Terminal);
        assert_eq!(view.sidebar_width, 190.0);
        assert!(view.selected_project_file.is_none());
    });

    let _ = std::fs::remove_dir_all(&root);
}
