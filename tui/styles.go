package tui

import (
	"github.com/charmbracelet/lipgloss"
)

var (
	ColorBg       = lipgloss.Color("#09090b")
	ColorFg       = lipgloss.Color("#e4e4e7")
	ColorMuted    = lipgloss.Color("#71717a")
	ColorBorder   = lipgloss.Color("#3f3f46")
	ColorAccent   = lipgloss.Color("#f59e0b")
	ColorGreen    = lipgloss.Color("#22c55e")
	ColorBlue     = lipgloss.Color("#3b82f6")
	ColorRed      = lipgloss.Color("#ef4444")
	ColorPurple   = lipgloss.Color("#a855f7")

	StyleApp = lipgloss.NewStyle().
			Background(ColorBg).
			Foreground(ColorFg)

	StyleHeader = lipgloss.NewStyle().
			Background(ColorBorder).
			Foreground(ColorAccent).
			Bold(true).
			Padding(0, 1)

	StyleSidebar = lipgloss.NewStyle().
			Border(lipgloss.RoundedBorder()).
			BorderForeground(ColorBorder)

	StyleTerminal = lipgloss.NewStyle().
			Border(lipgloss.RoundedBorder()).
			BorderForeground(ColorBorder)

	StyleTimeline = lipgloss.NewStyle().
			Border(lipgloss.RoundedBorder()).
			BorderForeground(ColorBorder)

	StyleTaskActive = lipgloss.NewStyle().
			Foreground(ColorAccent).
			Bold(true)

	StyleTaskInactive = lipgloss.NewStyle().
				Foreground(ColorMuted)

	StyleTaskStatus = lipgloss.NewStyle().
			Width(2).
			Align(lipgloss.Center)

	StyleEventCmd    = lipgloss.NewStyle().Foreground(ColorGreen)
	StyleEventTask   = lipgloss.NewStyle().Foreground(ColorBlue)
	StyleEventSystem = lipgloss.NewStyle().Foreground(ColorPurple)
	StyleTimestamp   = lipgloss.NewStyle().Foreground(ColorMuted)
)
