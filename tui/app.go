package tui

import (
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/hinshun/vt10x"
	"github.com/termforge/forge/internal/event"
	"github.com/termforge/forge/internal/session"
	"github.com/termforge/forge/internal/storage"
)

type termDataMsg struct{ data string }
type termErrorMsg struct{ err string }
type sessionCreatedMsg struct{ id string }
type snapshotLoadedMsg struct {
	snap *storage.WorkspaceSnapshot
}

type focusPanel int

const (
	focusTerminal focusPanel = iota
	focusSidebar
	focusTimeline
)

type App struct {
	width  int
	height int

	terminal viewport.Model
	focus    focusPanel

	sessMgr   *session.Manager
	db        *storage.DB
	vt        vt10x.Terminal
	sessionID string
	projectID string

	tasks    *TaskSidebar
	timeline *Timeline
	events   *event.Bus

	mu      sync.Mutex
	started bool
}

func NewApp(db *storage.DB, projectID string) *App {
	return &App{
		db:        db,
		sessMgr:   session.NewManager(),
		terminal:  viewport.New(80, 20),
		projectID: projectID,
		tasks:     NewTaskSidebar(db, projectID),
		timeline:  NewTimeline(db, projectID),
		events:    event.NewBus(),
	}
}

func (a *App) Init() tea.Cmd {
	return tea.Batch(
		tea.EnterAltScreen,
		a.loadSnapshot(),
	)
}

func (a *App) loadSnapshot() tea.Cmd {
	return func() tea.Msg {
		snap, _ := storage.GetLatestSnapshot(a.db, a.projectID)
		return snapshotLoadedMsg{snap: snap}
	}
}

func (a *App) createSession() tea.Cmd {
	return func() tea.Msg {
		sess, err := a.sessMgr.Create(a.projectID, "", "main", "", "")
		if err != nil {
			return termErrorMsg{err: err.Error()}
		}
		a.sessionID = sess.ID

		a.vt = vt10x.New(vt10x.WithSize(a.terminal.Width, a.terminal.Height))
		a.started = true

		a.events.Emit(event.MakeEvent(event.EventSessionStarted, a.projectID, map[string]string{
			"name":       "main",
			"session_id": sess.ID,
		}))

		storage.LogCommand(a.db, sess.ID, "", "", 0, 0)

		return sessionCreatedMsg{id: sess.ID}
	}
}

func (a *App) waitForOutput() tea.Cmd {
	return func() tea.Msg {
		sess, ok := a.sessMgr.Get(a.sessionID)
		if !ok {
			return termErrorMsg{err: "session not found"}
		}

		buf := make([]byte, 8192)
		n, err := sess.Pty.Read(buf)
		if err != nil {
			return termErrorMsg{err: err.Error()}
		}

		a.mu.Lock()
		a.vt.Write(buf[:n])
		content := a.vt.String()
		a.mu.Unlock()

		return termDataMsg{data: content}
	}
}

func (a *App) logCommand(input string) {
	taskID := ""
	if t := a.tasks.ActiveTask(); t != nil {
		taskID = t.ID
	}
	a.events.Emit(event.MakeEvent(event.EventCommandExecuted, a.projectID, map[string]string{
		"command":  input,
		"task_id":  taskID,
		"session":  a.sessionID,
	}))
	storage.LogCommand(a.db, a.sessionID, taskID, input, 0, 0)
}

func (a *App) saveSnapshot() {
	snap, err := storage.BuildSnapshot(a.db, a.projectID)
	if err != nil {
		return
	}
	storage.SaveSnapshot(a.db, snap)
}

func (a *App) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmds []tea.Cmd

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		a.width = msg.Width
		a.height = msg.Height
		a.layout()
		if a.started && a.vt != nil {
			a.vt.Resize(a.terminal.Width, a.terminal.Height)
			a.sessMgr.Resize(a.sessionID, a.terminal.Width, a.terminal.Height)
		}
		return a, nil

	case snapshotLoadedMsg:
		if msg.snap != nil {
			a.tasks.LoadTasks()
		}
		return a, a.createSession()

	case sessionCreatedMsg:
		a.sessionID = msg.id
		return a, a.waitForOutput()

	case termDataMsg:
		a.mu.Lock()
		a.terminal.SetContent(msg.data)
		a.mu.Unlock()
		a.terminal.GotoBottom()
		return a, a.waitForOutput()

	case termErrorMsg:
		a.mu.Lock()
		a.terminal.SetContent(fmt.Sprintf("[error] %s", msg.err))
		a.mu.Unlock()
		return a, nil

	case taskErrorMsg:
		a.mu.Lock()
		a.terminal.SetContent(fmt.Sprintf("[error] %s", msg.err))
		a.mu.Unlock()
		return a, nil

	case tea.KeyMsg:
		if a.tasks.creating {
			cmd := a.tasks.Update(msg)
			return a, cmd
		}

		switch msg.String() {
		case "ctrl+c":
			a.saveSnapshot()
			storage.MarkSessionInactive(a.db, a.sessionID)
			return a, tea.Quit

		case "tab":
			a.focus = (a.focus + 1) % 3
			a.layout()
			return a, nil

		case "shift+tab":
			a.focus = (a.focus + 2) % 3
			a.layout()
			return a, nil

		case "1":
			a.focus = focusTerminal
			return a, nil
		case "2":
			a.focus = focusSidebar
			return a, nil
		case "3":
			a.focus = focusTimeline
			a.timeline.LoadEvents()
			return a, nil
		}

		switch a.focus {
		case focusTerminal:
			if a.sessionID != "" {
				return a, a.sendInput(msg)
			}
		case focusSidebar:
			cmd := a.tasks.Update(msg)
			if cmd != nil {
				return a, cmd
			}
			if t := a.tasks.ActiveTask(); t != nil {
				a.timeline.SetFilter(t.ID)
			}
		case focusTimeline:
			a.timeline.Update(msg)
		}
	}

	return a, tea.Batch(cmds...)
}

func (a *App) sendInput(msg tea.KeyMsg) tea.Cmd {
	return func() tea.Msg {
		var data string
		switch msg.String() {
		case "enter":
			data = "\r"
			go a.logCommand("(enter)")
		case "backspace":
			data = "\x7f"
		case "tab":
			data = "\t"
		case "up":
			data = "\x1b[A"
		case "down":
			data = "\x1b[B"
		case "right":
			data = "\x1b[C"
		case "left":
			data = "\x1b[D"
		case "home":
			data = "\x1b[H"
		case "end":
			data = "\x1b[F"
		case "pgup":
			data = "\x1b[5~"
		case "pgdown":
			data = "\x1b[6~"
		default:
			data = msg.String()
			if len(data) == 1 {
				go a.logCommand(data)
			}
		}

		if err := a.sessMgr.WriteBytes(a.sessionID, []byte(data)); err != nil {
			return termErrorMsg{err: err.Error()}
		}
		return nil
	}
}

func (a *App) layout() {
	sidebarW := 24
	timelineH := 8
	termW := a.width - sidebarW - 4
	termH := a.height - timelineH - 4

	if termW < 20 {
		termW = 20
	}
	if termH < 5 {
		termH = 5
	}

	a.terminal.Width = termW
	a.terminal.Height = termH
	a.tasks.width = sidebarW - 2
	a.tasks.height = a.height - 4
	a.timeline.SetSize(termW, timelineH)
}

func (a *App) View() string {
	if !a.started {
		return lipgloss.NewStyle().Foreground(ColorMuted).Render("  starting...")
	}

	sidebarW := 24
	timelineH := 8
	termW := a.width - sidebarW - 4
	termH := a.height - timelineH - 4

	if termW < 20 {
		termW = 20
	}
	if termH < 5 {
		termH = 5
	}

	title := StyleHeader.Render(" TermForge ")

	shell := "powershell"
	connIcon := lipgloss.NewStyle().Foreground(ColorGreen).Render("●")
	taskTag := ""
	if t := a.tasks.ActiveTask(); t != nil {
		taskTag = lipgloss.NewStyle().Foreground(ColorAccent).Render(" [" + t.Title + "]")
	}
	status := lipgloss.NewStyle().Foreground(ColorMuted).Render(
		fmt.Sprintf("  project:%s  %s %s", a.projectID, connIcon, shell),
	) + taskTag

	sidebarBorderColor := ColorBorder
	if a.focus == focusSidebar {
		sidebarBorderColor = ColorAccent
	}
	sidebar := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(sidebarBorderColor).
		Width(sidebarW - 2).
		Height(a.height - 4).
		Render(a.tasks.View())

	termBorderColor := ColorBorder
	if a.focus == focusTerminal {
		termBorderColor = ColorAccent
	}
	a.mu.Lock()
	termContent := a.terminal.View()
	a.mu.Unlock()
	termBox := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(termBorderColor).
		Width(termW).
		Height(termH).
		Render(termContent)

	tlBorderColor := ColorBorder
	if a.focus == focusTimeline {
		tlBorderColor = ColorAccent
	}
	tlBox := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(tlBorderColor).
		Width(termW).
		Height(timelineH).
		Render(a.timeline.View())

	center := lipgloss.JoinVertical(lipgloss.Left, termBox, tlBox)
	main := lipgloss.JoinHorizontal(lipgloss.Top, sidebar, center)

	hints := []string{"tab:switch", "1/2/3:jump", "ctrl+c:quit"}
	if a.focus == focusSidebar {
		hints = []string{"n:new task", "j/k:navigate", "enter:select", "tab:switch"}
	} else if a.focus == focusTimeline {
		hints = []string{"j/k:scroll", "g/G:top/bottom", "f:filter", "tab:switch"}
	}
	bottom := lipgloss.NewStyle().
		Foreground(ColorMuted).
		Render(" " + strings.Join(hints, "  "))

	return lipgloss.JoinVertical(lipgloss.Left,
		lipgloss.JoinHorizontal(lipgloss.Left, title, status),
		main,
		bottom,
	)
}

func formatDuration(d time.Duration) string {
	if d < time.Second {
		return fmt.Sprintf("%dms", d.Milliseconds())
	}
	return fmt.Sprintf("%.1fs", d.Seconds())
}
