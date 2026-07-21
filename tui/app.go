package tui

import (
	"fmt"
	"strings"
	"sync"

	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/hinshun/vt10x"
	"github.com/termforge/forge/internal/session"
	"github.com/termforge/forge/internal/storage"
)

type termDataMsg struct{ data string }
type termErrorMsg struct{ err string }

type App struct {
	width  int
	height int

	terminal   viewport.Model
	sidebar    viewport.Model
	timeline   viewport.Model
	focusPanel int

	sessMgr   *session.Manager
	db        *storage.DB
	vt        vt10x.Terminal
	sessionID string
	projectID string
	ptyWidth  int
	ptyHeight int

	mu      sync.Mutex
	started bool
}

func NewApp(db *storage.DB, projectID string) *App {
	return &App{
		db:        db,
		sessMgr:   session.NewManager(),
		terminal:  viewport.New(80, 20),
		sidebar:   viewport.New(22, 24),
		timeline:  viewport.New(80, 8),
		projectID: projectID,
	}
}

func (a *App) Init() tea.Cmd {
	return tea.Batch(
		tea.EnterAltScreen,
		a.createSession(),
	)
}

func (a *App) createSession() tea.Cmd {
	return func() tea.Msg {
		sess, err := a.sessMgr.Create(a.projectID, "", "main", "", "")
		if err != nil {
			return termErrorMsg{err: err.Error()}
		}
		a.sessionID = sess.ID

		a.vt = vt10x.New(vt10x.WithSize(a.terminal.Width, a.terminal.Height))
		a.ptyWidth = a.terminal.Width
		a.ptyHeight = a.terminal.Height
		a.started = true

		return sessionCreatedMsg{id: sess.ID}
	}
}

type sessionCreatedMsg struct{ id string }

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

	case tea.KeyMsg:
		switch msg.String() {
		case "ctrl+c":
			return a, tea.Quit

		case "tab":
			a.focusPanel = (a.focusPanel + 1) % 3
			a.layout()
			return a, nil

		case "shift+tab":
			a.focusPanel = (a.focusPanel + 2) % 3
			a.layout()
			return a, nil

		case "1":
			a.focusPanel = 0
			return a, nil
		case "2":
			a.focusPanel = 1
			return a, nil
		case "3":
			a.focusPanel = 2
			return a, nil
		}

		if a.focusPanel == 0 && a.sessionID != "" {
			return a, a.sendInput(msg)
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
	a.sidebar.Width = sidebarW - 2
	a.sidebar.Height = a.height - 4
	a.timeline.Width = termW
	a.timeline.Height = timelineH
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
	status := lipgloss.NewStyle().Foreground(ColorMuted).Render(
		fmt.Sprintf("  project:%s  %s %s", a.projectID, connIcon, shell),
	)

	sidebarContent := a.viewSidebar()
	sidebar := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(ColorBorder).
		Width(sidebarW - 2).
		Height(a.height - 4).
		Render(sidebarContent)

	termBorderColor := ColorBorder
	if a.focusPanel == 0 {
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
	if a.focusPanel == 2 {
		tlBorderColor = ColorAccent
	}
	timelineContent := a.viewTimeline()
	tlBox := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(tlBorderColor).
		Width(termW).
		Height(timelineH).
		Render(timelineContent)

	center := lipgloss.JoinVertical(lipgloss.Left, termBox, tlBox)
	main := lipgloss.JoinHorizontal(lipgloss.Top, sidebar, center)

	bottom := lipgloss.NewStyle().
		Foreground(ColorMuted).
		Render(" tab:switch  1/2/3:jump  ctrl+c:quit ")

	return lipgloss.JoinVertical(lipgloss.Left,
		lipgloss.JoinHorizontal(lipgloss.Left, title, status),
		main,
		bottom,
	)
}

func (a *App) viewSidebar() string {
	var b strings.Builder
	b.WriteString(lipgloss.NewStyle().Bold(true).Foreground(ColorAccent).Render("PROJECT"))
	b.WriteString("\n\n")
	b.WriteString("  " + lipgloss.NewStyle().Foreground(ColorFg).Render("▸ ")+a.projectID)
	b.WriteString("\n\n\n")
	b.WriteString(lipgloss.NewStyle().Bold(true).Foreground(ColorAccent).Render("SESSION"))
	b.WriteString("\n\n")
	if a.sessionID != "" {
		b.WriteString("  " + lipgloss.NewStyle().Foreground(ColorGreen).Render("●")+" main (powershell)\n")
	} else {
		b.WriteString("  " + lipgloss.NewStyle().Foreground(ColorMuted).Render("○")+" starting...\n")
	}
	b.WriteString("\n\n")
	b.WriteString(lipgloss.NewStyle().Bold(true).Foreground(ColorAccent).Render("PANELS"))
	b.WriteString("\n\n")
	labels := []string{"Terminal", "Sidebar", "Timeline"}
	for i, l := range labels {
		marker := "○"
		style := lipgloss.NewStyle().Foreground(ColorMuted)
		if i == a.focusPanel {
			marker = "▸"
			style = lipgloss.NewStyle().Foreground(ColorAccent)
		}
		b.WriteString(fmt.Sprintf("  %s %s\n", marker, style.Render(l)))
	}
	return b.String()
}

func (a *App) viewTimeline() string {
	a.mu.Lock()
	defer a.mu.Unlock()

	if a.vt == nil {
		return lipgloss.NewStyle().Foreground(ColorMuted).Render("  waiting for output...")
	}

	cols, rows := a.vt.Size()
	var b strings.Builder
	for y := 0; y < rows; y++ {
		var line strings.Builder
		for x := 0; x < cols; x++ {
			g := a.vt.Cell(x, y)
			line.WriteRune(g.Char)
		}
		trimmed := strings.TrimRight(line.String(), " ")
		if trimmed != "" {
			b.WriteString("  " + trimmed + "\n")
		}
	}
	if b.Len() == 0 {
		return lipgloss.NewStyle().Foreground(ColorMuted).Render("  waiting for output...")
	}
	return b.String()
}
