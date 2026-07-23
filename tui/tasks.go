package tui

import (
	"fmt"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/termforge/forge/internal/storage"
)

type TaskItem struct {
	ID     string
	Title  string
	Status string
}

type TaskSidebar struct {
	tasks       []TaskItem
	activeIndex int
	width       int
	height      int
	db          *storage.DB
	projectID   string
	creating    bool
	inputBuffer string
}

func NewTaskSidebar(db *storage.DB, projectID string) *TaskSidebar {
	ts := &TaskSidebar{
		db:        db,
		projectID: projectID,
		width:     22,
		height:    20,
	}
	_ = ts.LoadTasks()
	return ts
}

func (ts *TaskSidebar) LoadTasks() error {
	dbTasks, err := ts.db.GetTasksByProject(ts.projectID)
	if err != nil {
		return err
	}
	ts.tasks = make([]TaskItem, 0, len(dbTasks))
	for _, t := range dbTasks {
		ts.tasks = append(ts.tasks, TaskItem{
			ID:     t.ID,
			Title:  t.Title,
			Status: t.Status,
		})
	}
	if ts.activeIndex >= len(ts.tasks) && len(ts.tasks) > 0 {
		ts.activeIndex = len(ts.tasks) - 1
	}
	return nil
}

func (ts *TaskSidebar) CreateTask(title string) error {
	id := fmt.Sprintf("task-%d", len(ts.tasks)+1)
	if err := ts.db.CreateTask(id, ts.projectID, title); err != nil {
		return err
	}
	return ts.LoadTasks()
}

func (ts *TaskSidebar) SetActiveTask(id string) error {
	for _, t := range ts.tasks {
		if t.Status == "active" {
			if err := ts.db.SetTaskStatus(t.ID, "backlog"); err != nil {
				return err
			}
		}
	}
	if err := ts.db.SetTaskStatus(id, "active"); err != nil {
		return err
	}
	return ts.LoadTasks()
}

func (ts *TaskSidebar) CompleteTask(id string) error {
	if err := ts.db.SetTaskStatus(id, "completed"); err != nil {
		return err
	}
	return ts.LoadTasks()
}

func (ts *TaskSidebar) Update(msg tea.KeyMsg) tea.Cmd {
	if ts.creating {
		return ts.updateCreating(msg)
	}
	return ts.updateNormal(msg)
}

func (ts *TaskSidebar) updateCreating(msg tea.KeyMsg) tea.Cmd {
	switch msg.String() {
	case "enter":
		if ts.inputBuffer != "" {
			err := ts.CreateTask(ts.inputBuffer)
			ts.inputBuffer = ""
			ts.creating = false
			if err != nil {
				return func() tea.Msg { return taskErrorMsg{err: err.Error()} }
			}
		}
		return nil

	case "escape":
		ts.creating = false
		ts.inputBuffer = ""
		return nil

	case "backspace":
		if len(ts.inputBuffer) > 0 {
			ts.inputBuffer = ts.inputBuffer[:len(ts.inputBuffer)-1]
		}
		return nil

	default:
		if len(msg.String()) == 1 {
			ts.inputBuffer += msg.String()
		}
		return nil
	}
}

func (ts *TaskSidebar) updateNormal(msg tea.KeyMsg) tea.Cmd {
	switch msg.String() {
	case "j", "down":
		if ts.activeIndex < len(ts.tasks)-1 {
			ts.activeIndex++
		}
	case "k", "up":
		if ts.activeIndex > 0 {
			ts.activeIndex--
		}
	case "n":
		ts.creating = true
		ts.inputBuffer = ""
	case "enter":
		if len(ts.tasks) > 0 {
			task := ts.tasks[ts.activeIndex]
			if task.Status == "completed" {
				return nil
			}
			err := ts.SetActiveTask(task.ID)
			if err != nil {
				return func() tea.Msg { return taskErrorMsg{err: err.Error()} }
			}
		}
	}
	return nil
}

type taskErrorMsg struct{ err string }

func (ts *TaskSidebar) View() string {
	var b strings.Builder

	header := lipgloss.NewStyle().
		Bold(true).
		Foreground(ColorAccent).
		Render("TASKS")
	b.WriteString(header)
	b.WriteString("\n\n")

	if len(ts.tasks) == 0 && !ts.creating {
		b.WriteString(lipgloss.NewStyle().Foreground(ColorMuted).Render("  no tasks yet"))
		b.WriteString("\n")
	} else {
		for i, task := range ts.tasks {
			indicator := ts.renderStatusIndicator(task.Status)
			titleStyle := lipgloss.NewStyle().Foreground(ColorFg)
			if task.Status == "active" {
				titleStyle = StyleTaskActive
			} else if task.Status == "completed" {
				titleStyle = lipgloss.NewStyle().Foreground(ColorMuted).Strikethrough(true)
			}

			cursor := "  "
			if i == ts.activeIndex {
				cursor = lipgloss.NewStyle().Foreground(ColorAccent).Render("▸ ")
			}

			line := fmt.Sprintf("%s%s %s", cursor, indicator, titleStyle.Render(task.Title))
			b.WriteString(line)
			b.WriteString("\n")
		}
	}

	if ts.creating {
		b.WriteString("\n")
		prompt := lipgloss.NewStyle().Foreground(ColorAccent).Render("> ")
		cursor := lipgloss.NewStyle().Foreground(ColorAccent).Render("█")
		b.WriteString(prompt + ts.inputBuffer + cursor)
		b.WriteString("\n")
	}

	b.WriteString("\n")
	hint := lipgloss.NewStyle().Foreground(ColorMuted).Render(" n:new  j/k:navigate  enter:select")
	b.WriteString(hint)

	return b.String()
}

func (ts *TaskSidebar) renderStatusIndicator(status string) string {
	switch status {
	case "active":
		return lipgloss.NewStyle().Foreground(ColorGreen).Render("●")
	case "backlog":
		return lipgloss.NewStyle().Foreground(ColorMuted).Render("○")
	case "paused":
		return lipgloss.NewStyle().Foreground(ColorAccent).Render("◐")
	case "completed":
		return lipgloss.NewStyle().Foreground(ColorBlue).Render("✓")
	default:
		return lipgloss.NewStyle().Foreground(ColorMuted).Render("○")
	}
}

func (ts *TaskSidebar) ActiveTask() *TaskItem {
	for i, task := range ts.tasks {
		if task.Status == "active" {
			return &ts.tasks[i]
		}
	}
	return nil
}
