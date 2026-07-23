package main

import (
	"fmt"
	"log"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"

	"github.com/termforge/forge/internal/ipc"
	"github.com/termforge/forge/internal/storage"
)

func main() {
	log.SetFlags(log.LstdFlags | log.Lshortfile)

	if len(os.Args) < 2 {
		fmt.Fprintf(os.Stderr, "Usage: forge <project-dir>\n")
		os.Exit(1)
	}

	projectDir := os.Args[1]

	// Resolve project dir
	absDir, err := filepath.Abs(projectDir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)
		os.Exit(1)
	}

	// Data dir for this project
	dataDir := storage.DBPath(absDir)

	// Open/create database
	db, err := storage.Open(dataDir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error opening database: %v\n", err)
		os.Exit(1)
	}
	defer db.Close()

	// Get or create project
	projectID := storage.Slugify(absDir)
	if err := db.EnsureProject(projectID, absDir); err != nil {
		fmt.Fprintf(os.Stderr, "error creating project: %v\n", err)
		os.Exit(1)
	}

	// Create and start daemon
	d := ipc.NewDaemon(db, projectID, absDir)
	if err := d.Start(); err != nil {
		fmt.Fprintf(os.Stderr, "error starting daemon: %v\n", err)
		os.Exit(1)
	}
	defer d.Stop()

	// Write port file so the UI can connect
	portFile := filepath.Join(dataDir, "daemon.port")
	if err := os.WriteFile(portFile, []byte(fmt.Sprintf("%d", d.Port())), 0644); err != nil {
		fmt.Fprintf(os.Stderr, "error writing port file: %v\n", err)
		os.Exit(1)
	}

	log.Printf("[main] daemon running on port %d, port file: %s", d.Port(), portFile)

	// Wait for signals
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	<-sigCh

	log.Println("[main] shutting down")
	os.Remove(portFile)
}
