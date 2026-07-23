package tui

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/termforge/forge/internal/storage"
)

type TimelineEvent struct {
	ID        string
	Type      string
	TaskID    string
	Message   string
	TaskTitle string
	Timestamp time.Time
	Icon      string
}

func iconForType(t string) string {
	switch {
	case strings.HasPrefix(t, "command"):
		return "⌘"
	case strings.HasPrefix(t, "session"):
		return "●"
	case strings.HasPrefix(t, "task"):
		return "◆"
	case strings.HasPrefix(t, "file"):
		return "¶"
	default:
		return "⚙"
	}
}

type Timeline struct {
	events      []TimelineEvent
	scrollOffset int
	width       int
	height      int
	db          *storage.DB
	projectID   string
	filterTaskID string
	taskIDs     []string
	taskNames   map[string]string
}

func NewTimeline(db *storage.DB, projectID string) *Timeline {
	return &Timeline{
		db:        db,
		projectID: projectID,
		taskNames: make(map[string]string),
	}
}

func (tl *Timeline) LoadEvents() error {
	events, err := tl.db.GetRecentEvents(tl.projectID, 200)
	if err != nil {
		return err
	}

	tasks, _ := tl.db.GetTasksByProject(tl.projectID)
	taskIDs := make([]string, 0, len(tasks))
	taskNames := make(map[string]string)
	for _, t := range tasks {
		taskIDs = append(taskIDs, t.ID)
		taskNames[t.ID] = t.Title
	}
	tl.taskIDs = taskIDs
	tl.taskNames = taskNames

	tl.events = make([]TimelineEvent, 0, len(events))
	for i := len(events) - 1; i >= 0; i-- {
		e := events[i]
		te := TimelineEvent{
			ID:        e.ID,
			Type:      string(e.Type),
			TaskID:    e.TaskID,
			Timestamp: e.CreatedAt,
			Icon:      iconForType(string(e.Type)),
		}

		if e.TaskID != "" {
			te.TaskTitle = tl.taskNames[e.TaskID]
		}

		var payload map[string]interface{}
		json.Unmarshal(e.Payload, &payload)

		switch e.Type {
		case "session.started":
			if name, ok := payload["name"].(string); ok {
				te.Message = "Session started: " + name
			} else {
				te.Message = "Session started"
			}
		case "command.executed":
			if cmd, ok := payload["input_command"].(string); ok {
				te.Message = cmd
			} else if cmd, ok := payload["command"].(string); ok {
				te.Message = cmd
			} else {
				te.Message = "Command executed"
			}
		case "task.activated":
			if tid, ok := payload["task_id"].(string); ok {
				if name, ok := tl.taskNames[tid]; ok {
					te.Message = "Task activated: " + name
				} else {
					te.Message = "Task activated: " + tid
				}
			} else {
				te.Message = "Task activated"
			}
		case "task.created":
			if title, ok := payload["title"].(string); ok {
				te.Message = "Task created: " + title
			} else {
				te.Message = "Task created"
			}
		case "project.opened":
			if dir, ok := payload["root_directory"].(string); ok {
				te.Message = "Project opened: " + dir
			} else {
				te.Message = "Project opened"
			}
		case "session.stopped":
			te.Message = "Session stopped"
		case "task.completed":
			te.Message = "Task completed"
		case "note.created":
			te.Message = "Note created"
		default:
			te.Message = string(e.Type)
		}

		tl.events = append(tl.events, te)
	}

	tl.scrollOffset = 0
	return nil
}

func (tl *Timeline) SetFilter(taskID string) {
	tl.filterTaskID = taskID
	tl.scrollOffset = 0
}

func (tl *Timeline) filteredEvents() []TimelineEvent {
	if tl.filterTaskID == "" {
		return tl.events
	}
	var filtered []TimelineEvent
	for _, e := range tl.events {
		if e.TaskID == tl.filterTaskID {
			filtered = append(filtered, e)
		}
	}
	return filtered
}

func (tl *Timeline) ScrollUp() {
	if tl.scrollOffset > 0 {
		tl.scrollOffset--
	}
}

func (tl *Timeline) ScrollDown() {
	events := tl.filteredEvents()
	viewportH := tl.visibleRows()
	if tl.scrollOffset < len(events)-viewportH {
		tl.scrollOffset++
	}
}

func (tl *Timeline) visibleRows() int {
	if tl.height <= 2 {
		return 0
	}
	return tl.height - 2
}

func (tl *Timeline) Update(msg tea.KeyMsg) tea.Cmd {
	switch msg.String() {
	case "g":
		tl.scrollOffset = 0
	case "G":
		events := tl.filteredEvents()
		viewportH := tl.visibleRows()
		if len(events) > viewportH {
			tl.scrollOffset = len(events) - viewportH
		} else {
			tl.scrollOffset = 0
		}
	case "k", "up":
		tl.ScrollUp()
	case "j", "down":
		tl.ScrollDown()
	case "f":
		if len(tl.taskIDs) == 0 {
			return nil
		}
		currentIdx := -1
		for i, tid := range tl.taskIDs {
			if tid == tl.filterTaskID {
				currentIdx = i
				break
			}
		}
		nextIdx := currentIdx + 1
		if nextIdx >= len(tl.taskIDs) {
			tl.filterTaskID = ""
		} else {
			tl.filterTaskID = tl.taskIDs[nextIdx]
		}
		tl.scrollOffset = 0
	}
	return nil
}

func (tl *Timeline) SetSize(w, h int) {
	tl.width = w
	tl.height = h
}

func (tl *Timeline) View() string {
	events := tl.filteredEvents()
	viewportH := tl.visibleRows()

	var b strings.Builder

	headerStyle := lipgloss.NewStyle().
		Bold(true).
		Foreground(ColorAccent).
		PaddingBottom(0)
	taskTagStyle := lipgloss.NewStyle().Foreground(ColorAccent).Bold(true)
	header := headerStyle.Render("TIMELINE")
	if tl.filterTaskID != "" {
		filterTag := taskTagStyle.Render(" [" + tl.taskNames[tl.filterTaskID] + "]")
		header += filterTag
	}
	b.WriteString(header)
	b.WriteString("\n")

	if len(events) == 0 {
		mutedStyle := lipgloss.NewStyle().Foreground(ColorMuted)
		b.WriteString(mutedStyle.Render("  no events"))
		return b.String()
	}

	if tl.scrollOffset > 0 && tl.scrollOffset+viewportH < len(events) {
		// scrolling in middle
	} else if tl.scrollOffset+viewportH >= len(events) {
		tl.scrollOffset = len(events) - viewportH
		if tl.scrollOffset < 0 {
			tl.scrollOffset = 0
		}
	}

	end := tl.scrollOffset + viewportH
	if end > len(events) {
		end = len(events)
	}
	visible := events[tl.scrollOffset:end]

	timestampStyle := lipgloss.NewStyle().Foreground(ColorMuted)
	dimStyle := lipgloss.NewStyle().Foreground(ColorMuted)

	cmdStyle := lipgloss.NewStyle().Foreground(ColorGreen)
	sessionStyle := lipgloss.NewStyle().Foreground(ColorBlue)
	taskStyle := lipgloss.NewStyle().Foreground(ColorAccent)
	fileStyle := lipgloss.NewStyle().Foreground(ColorPurple)
	mutedIconStyle := lipgloss.NewStyle().Foreground(ColorMuted)

	for _, e := range visible {
		ts := timestampStyle.Render(e.Timestamp.Format("15:04:05"))

		var iconStyled string
		switch e.Icon {
		case "⌘":
			iconStyled = cmdStyle.Render(e.Icon)
		case "●":
			iconStyled = sessionStyle.Render(e.Icon)
		case "◆":
			iconStyled = taskStyle.Render(e.Icon)
		case "¶":
			iconStyled = fileStyle.Render(e.Icon)
		default:
			iconStyled = mutedIconStyle.Render(e.Icon)
		}

		var tag string
		if e.TaskTitle != "" {
			tag = taskTagStyle.Render(" ["+e.TaskTitle+"]") + " "
		}

		msgStyle := lipgloss.NewStyle().Foreground(ColorFg)
		msg := msgStyle.Render(e.Message)

		line := fmt.Sprintf("  %s %s %s%s", ts, iconStyled, tag, msg)
		b.WriteString(line)
		b.WriteString("\n")
	}

	if len(events) > viewportH {
		scrollTotal := len(events)
		scrollPos := tl.scrollOffset + viewportH
		if scrollPos > scrollTotal {
			scrollPos = scrollTotal
		}
		bar := fmt.Sprintf("%d/%d", scrollPos, scrollTotal)
		scrollIndicator := dimStyle.Render(bar)
		// pad to width
		padding := tl.width - len(bar) - 10
		if padding > 0 {
			b.WriteString(strings.Repeat(" ", padding))
			b.WriteString(scrollIndicator)
		}
	}

	hintStyle := lipgloss.NewStyle().Foreground(ColorMuted)
	hint := hintStyle.Render("  j/k:scroll  g/G:top/bottom  f:filter")
	b.WriteString(hint)

	return b.String()
}


