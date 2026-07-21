package main

import (
	"flag"
	"fmt"
	"os"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/termforge/forge/internal/storage"
	"github.com/termforge/forge/tui"
)

func main() {
	projectID := flag.String("project", "default", "project identifier")
	dataDir := flag.String("data-dir", "", "data directory (default: ~/.forge)")
	flag.Parse()

	if *dataDir == "" {
		home, _ := os.UserHomeDir()
		*dataDir = home + "/.forge"
	}
	os.MkdirAll(*dataDir, 0o755)

	db, err := storage.NewSQLite(*dataDir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "database error: %v\n", err)
		os.Exit(1)
	}
	defer db.Close()

	if err := db.RunMigrations(); err != nil {
		fmt.Fprintf(os.Stderr, "migration error: %v\n", err)
		os.Exit(1)
	}

	db.UpsertProject(*projectID, *projectID, ".")

	p := tea.NewProgram(
		tui.NewApp(db, *projectID),
		tea.WithAltScreen(),
	)

	if _, err := p.Run(); err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)
		os.Exit(1)
	}
}
