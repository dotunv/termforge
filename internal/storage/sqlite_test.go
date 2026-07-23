package storage

import (
	"testing"

	"github.com/termforge/forge/internal/event"
)

func setupTestDB(t *testing.T) *DB {
	t.Helper()
	dir := t.TempDir()
	db, err := NewSQLite(dir)
	if err != nil {
		t.Fatalf("failed to create test db: %v", err)
	}
	if err := db.RunMigrations(); err != nil {
		t.Fatalf("failed to run migrations: %v", err)
	}
	t.Cleanup(func() { db.Close() })
	return db
}

func TestSQLiteMigrations(t *testing.T) {
	db := setupTestDB(t)

	tables := []string{"projects", "tasks", "sessions", "commands", "events", "knowledge_nodes", "workspace_snapshots"}
	for _, table := range tables {
		var count int
		err := db.conn.QueryRow("SELECT COUNT(*) FROM " + table).Scan(&count)
		if err != nil {
			t.Fatalf("table %s not accessible: %v", table, err)
		}
	}
}

func TestUpsertProject(t *testing.T) {
	db := setupTestDB(t)

	err := db.UpsertProject("p1", "Test Project", "/tmp/test")
	if err != nil {
		t.Fatalf("UpsertProject failed: %v", err)
	}

	var name string
	err = db.conn.QueryRow("SELECT name FROM projects WHERE id = ?", "p1").Scan(&name)
	if err != nil {
		t.Fatalf("project not found: %v", err)
	}
	if name != "Test Project" {
		t.Fatalf("expected Test Project, got %s", name)
	}
}

func TestCreateAndGetTask(t *testing.T) {
	db := setupTestDB(t)
	db.UpsertProject("p1", "Test", "/tmp")

	err := db.CreateTask("t1", "p1", "Test Task")
	if err != nil {
		t.Fatalf("CreateTask failed: %v", err)
	}

	tasks, err := db.GetTasksByProject("p1")
	if err != nil {
		t.Fatalf("GetTasksByProject failed: %v", err)
	}
	if len(tasks) != 1 {
		t.Fatalf("expected 1 task, got %d", len(tasks))
	}
	if tasks[0].Title != "Test Task" {
		t.Fatalf("expected Test Task, got %s", tasks[0].Title)
	}
	if tasks[0].Status != "backlog" {
		t.Fatalf("expected backlog, got %s", tasks[0].Status)
	}
}

func TestSetTaskStatus(t *testing.T) {
	db := setupTestDB(t)
	db.UpsertProject("p1", "Test", "/tmp")
	db.CreateTask("t1", "p1", "Task")

	err := db.SetTaskStatus("t1", "active")
	if err != nil {
		t.Fatalf("SetTaskStatus failed: %v", err)
	}

	task, err := db.GetActiveTaskForProject("p1")
	if err != nil {
		t.Fatalf("GetActiveTaskForProject failed: %v", err)
	}
	if task.ID != "t1" {
		t.Fatalf("expected t1, got %s", task.ID)
	}
}

func TestInsertAndGetEvents(t *testing.T) {
	db := setupTestDB(t)
	db.UpsertProject("p1", "Test", "/tmp")

	e := event.MakeEvent(event.EventSessionStarted, "p1", map[string]string{"name": "test"})
	err := db.InsertEvent(e)
	if err != nil {
		t.Fatalf("InsertEvent failed: %v", err)
	}

	events, err := db.GetRecentEvents("p1", 10)
	if err != nil {
		t.Fatalf("GetRecentEvents failed: %v", err)
	}
	if len(events) != 1 {
		t.Fatalf("expected 1 event, got %d", len(events))
	}
	if events[0].Type != event.EventSessionStarted {
		t.Fatalf("expected session.started, got %s", events[0].Type)
	}
}

func TestInsertCommand(t *testing.T) {
	db := setupTestDB(t)
	db.UpsertProject("p1", "Test", "/tmp")

	db.conn.Exec(
		`INSERT INTO sessions (id, project_id, name, current_working_dir) VALUES (?, ?, ?, ?)`,
		"s1", "p1", "main", "/tmp",
	)

	err := LogCommand(db, "s1", "", "ls -la", 150, 0)
	if err != nil {
		t.Fatalf("LogCommand failed: %v", err)
	}

	commands, err := GetRecentCommands(db, "p1", 10)
	if err != nil {
		t.Fatalf("GetRecentCommands failed: %v", err)
	}
	if len(commands) != 1 {
		t.Fatalf("expected 1 command, got %d", len(commands))
	}
	if commands[0].InputCommand != "ls -la" {
		t.Fatalf("expected ls -la, got %s", commands[0].InputCommand)
	}
	if commands[0].DurationMs != 150 {
		t.Fatalf("expected 150ms, got %d", commands[0].DurationMs)
	}
}

func TestMarkSessionInactive(t *testing.T) {
	db := setupTestDB(t)
	db.UpsertProject("p1", "Test", "/tmp")

	db.conn.Exec(
		`INSERT INTO sessions (id, project_id, name, current_working_dir, is_active) VALUES (?, ?, ?, ?, 1)`,
		"s1", "p1", "main", "/tmp",
	)

	err := MarkSessionInactive(db, "s1")
	if err != nil {
		t.Fatalf("MarkSessionInactive failed: %v", err)
	}

	var isActive int
	db.conn.QueryRow("SELECT is_active FROM sessions WHERE id = ?", "s1").Scan(&isActive)
	if isActive != 0 {
		t.Fatalf("expected is_active=0, got %d", isActive)
	}
}

func TestSnapshot(t *testing.T) {
	db := setupTestDB(t)
	db.UpsertProject("p1", "Test", "/tmp")

	snap, err := BuildSnapshot(db, "p1")
	if err != nil {
		t.Fatalf("BuildSnapshot failed: %v", err)
	}
	if snap.ProjectID != "p1" {
		t.Fatalf("expected p1, got %s", snap.ProjectID)
	}

	err = SaveSnapshot(db, snap)
	if err != nil {
		t.Fatalf("SaveSnapshot failed: %v", err)
	}

	loaded, err := GetLatestSnapshot(db, "p1")
	if err != nil {
		t.Fatalf("GetLatestSnapshot failed: %v", err)
	}
	if loaded == nil {
		t.Fatal("expected non-nil snapshot")
	}
	if loaded.ID != snap.ID {
		t.Fatalf("expected %s, got %s", snap.ID, loaded.ID)
	}

	has, err := HasSnapshot(db, "p1")
	if err != nil {
		t.Fatalf("HasSnapshot failed: %v", err)
	}
	if !has {
		t.Fatal("expected HasSnapshot to return true")
	}

	err = ClearSnapshots(db, "p1")
	if err != nil {
		t.Fatalf("ClearSnapshots failed: %v", err)
	}

	has, _ = HasSnapshot(db, "p1")
	if has {
		t.Fatal("expected HasSnapshot to return false after clear")
	}
}

func TestGetRecentEventsLimit(t *testing.T) {
	db := setupTestDB(t)
	db.UpsertProject("p1", "Test", "/tmp")

	for i := 0; i < 10; i++ {
		e := event.MakeEvent(event.EventSessionStarted, "p1", nil)
		db.InsertEvent(e)
	}

	events, err := db.GetRecentEvents("p1", 5)
	if err != nil {
		t.Fatalf("GetRecentEvents failed: %v", err)
	}
	if len(events) != 5 {
		t.Fatalf("expected 5 events, got %d", len(events))
	}
}

func TestGetTasksByProjectEmpty(t *testing.T) {
	db := setupTestDB(t)
	db.UpsertProject("p1", "Test", "/tmp")

	tasks, err := db.GetTasksByProject("p1")
	if err != nil {
		t.Fatalf("GetTasksByProject failed: %v", err)
	}
	if len(tasks) != 0 {
		t.Fatalf("expected 0 tasks, got %d", len(tasks))
	}
}
