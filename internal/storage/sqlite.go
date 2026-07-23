package storage

import (
	"crypto/sha256"
	"database/sql"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/termforge/forge/internal/event"

	_ "modernc.org/sqlite"
)

func DBPath(projectDir string) string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".local", "share", "termforge", Slugify(projectDir))
}

func Slugify(s string) string {
	s = strings.ReplaceAll(s, "\\", "/")
	s = strings.TrimRight(s, "/")
	s = strings.ToLower(s)
	s = strings.ReplaceAll(s, " ", "-")
	s = strings.ReplaceAll(s, ":", "-")
	s = strings.ReplaceAll(s, ".", "-")

	// For Windows paths like C:/Users/... we want "c-users-..."
	if len(s) > 2 && s[1] == ':' {
		s = s[:1] + s[2:]
	}

	h := sha256.Sum256([]byte(s))
	return fmt.Sprintf("%x", h[:8])
}

type DB struct {
	conn *sql.DB
}

func NewSQLite(dataDir string) (*DB, error) {
	if err := os.MkdirAll(dataDir, 0o755); err != nil {
		return nil, fmt.Errorf("create data dir: %w", err)
	}

	dbPath := filepath.Join(dataDir, "forge.db")
	conn, err := sql.Open("sqlite", dbPath+"?_journal_mode=WAL&_busy_timeout=5000")
	if err != nil {
		return nil, fmt.Errorf("open database: %w", err)
	}

	conn.SetMaxOpenConns(1)

	if err := conn.Ping(); err != nil {
		conn.Close()
		return nil, fmt.Errorf("ping database: %w", err)
	}

	db := &DB{conn: conn}
	if err := db.RunMigrations(); err != nil {
		conn.Close()
		return nil, fmt.Errorf("run migrations: %w", err)
	}

	return db, nil
}

func Open(dataDir string) (*DB, error) {
	return NewSQLite(dataDir)
}

func (db *DB) Close() error {
	return db.conn.Close()
}

func (db *DB) Conn() *sql.DB {
	return db.conn
}

func (db *DB) RunMigrations() error {
	migrations := []string{
		`CREATE TABLE IF NOT EXISTS projects (
			id TEXT PRIMARY KEY,
			name TEXT NOT NULL,
			root_directory TEXT NOT NULL,
			created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
		)`,
		`CREATE TABLE IF NOT EXISTS tasks (
			id TEXT PRIMARY KEY,
			project_id TEXT NOT NULL,
			title TEXT NOT NULL,
			status TEXT CHECK(status IN ('backlog', 'active', 'paused', 'completed')) DEFAULT 'backlog',
			created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
			FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
		)`,
		`CREATE TABLE IF NOT EXISTS sessions (
			id TEXT PRIMARY KEY,
			project_id TEXT NOT NULL,
			task_id TEXT,
			name TEXT NOT NULL,
			current_working_dir TEXT NOT NULL,
			is_active INTEGER DEFAULT 1,
			created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
			FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE,
			FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE SET NULL
		)`,
		`CREATE TABLE IF NOT EXISTS commands (
			id TEXT PRIMARY KEY,
			session_id TEXT NOT NULL,
			task_id TEXT,
			input_command TEXT NOT NULL,
			exit_code INTEGER,
			execution_duration_ms INTEGER,
			started_at TIMESTAMP NOT NULL,
			FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
			FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE SET NULL
		)`,
		`CREATE TABLE IF NOT EXISTS events (
			id TEXT PRIMARY KEY,
			project_id TEXT NOT NULL,
			task_id TEXT,
			session_id TEXT,
			event_type TEXT NOT NULL,
			payload TEXT NOT NULL DEFAULT '{}',
			created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
			FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
		)`,
		`CREATE TABLE IF NOT EXISTS knowledge_nodes (
			id TEXT PRIMARY KEY,
			project_id TEXT NOT NULL,
			title TEXT NOT NULL,
			content TEXT NOT NULL,
			node_type TEXT CHECK(node_type IN ('decision', 'note', 'architecture_record')) NOT NULL,
			created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
			FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
		)`,
		`CREATE TABLE IF NOT EXISTS workspace_snapshots (
			id TEXT PRIMARY KEY,
			project_id TEXT NOT NULL,
			snapshot_data TEXT NOT NULL,
			created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
			FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
		)`,
		`CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id)`,
		`CREATE INDEX IF NOT EXISTS idx_events_task ON events(task_id)`,
		`CREATE INDEX IF NOT EXISTS idx_events_created ON events(created_at)`,
		`CREATE INDEX IF NOT EXISTS idx_commands_session ON commands(session_id)`,
		`CREATE INDEX IF NOT EXISTS idx_commands_task ON commands(task_id)`,
		`CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_id)`,
		`CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status)`,
	}

	for _, m := range migrations {
		if _, err := db.conn.Exec(m); err != nil {
			return fmt.Errorf("migration failed: %w", err)
		}
	}

	return nil
}

func (db *DB) InsertEvent(e event.Event) error {
	_, err := db.conn.Exec(
		`INSERT INTO events (id, project_id, task_id, session_id, event_type, payload, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?)`,
		e.ID, e.ProjectID, e.TaskID, e.SessionID, string(e.Type), string(e.Payload), e.CreatedAt,
	)
	return err
}

func (db *DB) GetRecentEvents(projectID string, limit int) ([]event.Event, error) {
	if limit <= 0 {
		limit = 50
	}
	rows, err := db.conn.Query(
		`SELECT id, project_id, COALESCE(task_id,''), COALESCE(session_id,''),
		        event_type, payload, created_at
		 FROM events WHERE project_id = ?
		 ORDER BY created_at DESC LIMIT ?`,
		projectID, limit,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var events []event.Event
	for rows.Next() {
		var e event.Event
		var evType string
		var payload string
		var createdAt time.Time
		if err := rows.Scan(&e.ID, &e.ProjectID, &e.TaskID, &e.SessionID, &evType, &payload, &createdAt); err != nil {
			return nil, err
		}
		e.Type = event.EventType(evType)
		e.Payload = []byte(payload)
		e.CreatedAt = createdAt
		events = append(events, e)
	}
	return events, nil
}

func (db *DB) UpsertProject(id, name, rootDir string) error {
	_, err := db.conn.Exec(
		`INSERT OR REPLACE INTO projects (id, name, root_directory) VALUES (?, ?, ?)`,
		id, name, rootDir,
	)
	return err
}

func (db *DB) EnsureProject(id, rootDir string) error {
	_, err := db.conn.Exec(
		`INSERT OR IGNORE INTO projects (id, name, root_directory) VALUES (?, ?, ?)`,
		id, id, rootDir,
	)
	return err
}

func (db *DB) CreateTask(id, projectID, title string) error {
	_, err := db.conn.Exec(
		`INSERT INTO tasks (id, project_id, title, status) VALUES (?, ?, ?, 'backlog')`,
		id, projectID, title,
	)
	return err
}

func (db *DB) SetTaskStatus(id, status string) error {
	_, err := db.conn.Exec(`UPDATE tasks SET status = ? WHERE id = ?`, status, id)
	return err
}

func (db *DB) GetTasksByProject(projectID string) ([]Task, error) {
	rows, err := db.conn.Query(
		`SELECT id, project_id, title, status FROM tasks WHERE project_id = ? ORDER BY created_at DESC`,
		projectID,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var tasks []Task
	for rows.Next() {
		var t Task
		if err := rows.Scan(&t.ID, &t.ProjectID, &t.Title, &t.Status); err != nil {
			return nil, err
		}
		tasks = append(tasks, t)
	}
	return tasks, nil
}

func (db *DB) GetActiveTaskForProject(projectID string) (*Task, error) {
	var t Task
	err := db.conn.QueryRow(
		`SELECT id, project_id, title, status FROM tasks WHERE project_id = ? AND status = 'active' LIMIT 1`,
		projectID,
	).Scan(&t.ID, &t.ProjectID, &t.Title, &t.Status)
	if err != nil {
		return nil, err
	}
	return &t, nil
}

type Task struct {
	ID        string
	ProjectID string
	Title     string
	Status    string
}
