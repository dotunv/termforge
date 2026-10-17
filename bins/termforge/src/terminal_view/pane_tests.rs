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

#[gpui::test]
async fn workbench_starts_project_first_and_terminal_can_maximize(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.read_with(cx, |view, _| {
        assert!(
            !view.show_tab_strip,
            "terminal tabs do not masquerade as workspace artifacts"
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
    redraw(cx);
    view.read_with(cx, |v, _| {
        assert!(v.show_context_panel);
        assert_eq!(v.inspector, Inspector::Diff("a.txt".into()));
        assert!(
            v.diff_lines.iter().any(|l| l == "+hello"),
            "{:?}",
            v.diff_lines
        );
    });
    let _ = std::fs::remove_dir_all(&repo);
}
