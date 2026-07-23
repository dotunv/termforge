package storage

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"time"

	"github.com/google/uuid"
)

type SnapshotCommand struct {
	Input     string    `json:"input"`
	Timestamp time.Time `json:"timestamp"`
	TaskTitle string    `json:"task_title"`
}

type SnapshotEvent struct {
	Type      string    `json:"type"`
	Message   string    `json:"message"`
	Timestamp time.Time `json:"timestamp"`
}

type WorkspaceSnapshot struct {
	ID                 string            `json:"id"`
	ProjectID          string            `json:"project_id"`
	ActiveTaskID       string            `json:"active_task_id"`
	ActiveSessionName  string            `json:"active_session_name"`
	CWD                string            `json:"cwd"`
	RecentCommands     []SnapshotCommand `json:"recent_commands"`
	RecentEvents       []SnapshotEvent   `json:"recent_events"`
	CreatedAt          time.Time         `json:"created_at"`
}

func SaveSnapshot(db *DB, snap *WorkspaceSnapshot) error {
	if snap.ID == "" {
		snap.ID = uuid.New().String()
	}
	if snap.CreatedAt.IsZero() {
		snap.CreatedAt = time.Now()
	}

	data, err := json.Marshal(snap)
	if err != nil {
		return fmt.Errorf("marshal snapshot: %w", err)
	}

	_, err = db.conn.Exec(
		`INSERT INTO workspace_snapshots (id, project_id, snapshot_data, created_at)
		 VALUES (?, ?, ?, ?)`,
		snap.ID, snap.ProjectID, string(data), snap.CreatedAt,
	)
	return err
}

func GetLatestSnapshot(db *DB, projectID string) (*WorkspaceSnapshot, error) {
	var snapData string
	var snap WorkspaceSnapshot

	err := db.conn.QueryRow(
		`SELECT id, project_id, snapshot_data, created_at
		 FROM workspace_snapshots
		 WHERE project_id = ?
		 ORDER BY created_at DESC LIMIT 1`,
		projectID,
	).Scan(&snap.ID, &snap.ProjectID, &snapData, &snap.CreatedAt)
	if err == sql.ErrNoRows {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}

	if err := json.Unmarshal([]byte(snapData), &snap); err != nil {
		return nil, fmt.Errorf("unmarshal snapshot: %w", err)
	}

	return &snap, nil
}

func BuildSnapshot(db *DB, projectID string) (*WorkspaceSnapshot, error) {
	snap := &WorkspaceSnapshot{
		ID:        uuid.New().String(),
		ProjectID: projectID,
		CreatedAt: time.Now(),
	}

	activeTask, err := db.GetActiveTaskForProject(projectID)
	if err != nil && err != sql.ErrNoRows {
		return nil, fmt.Errorf("get active task: %w", err)
	}
	if activeTask != nil {
		snap.ActiveTaskID = activeTask.ID
	}

	var activeSessionName, cwd string
	err = db.conn.QueryRow(
		`SELECT name, current_working_dir
		 FROM sessions
		 WHERE project_id = ? AND is_active = 1
		 ORDER BY created_at DESC LIMIT 1`,
		projectID,
	).Scan(&activeSessionName, &cwd)
	if err != nil && err != sql.ErrNoRows {
		return nil, fmt.Errorf("get active session: %w", err)
	}
	snap.ActiveSessionName = activeSessionName
	snap.CWD = cwd

	rows, err := db.conn.Query(
		`SELECT c.input_command, c.started_at, COALESCE(t.title, '')
		 FROM commands c
		 LEFT JOIN tasks t ON c.task_id = t.id
		 INNER JOIN sessions s ON c.session_id = s.id
		 WHERE s.project_id = ?
		 ORDER BY c.started_at DESC LIMIT 20`,
		projectID,
	)
	if err != nil {
		return nil, fmt.Errorf("query commands: %w", err)
	}
	defer rows.Close()

	for rows.Next() {
		var cmd SnapshotCommand
		if err := rows.Scan(&cmd.Input, &cmd.Timestamp, &cmd.TaskTitle); err != nil {
			return nil, fmt.Errorf("scan command: %w", err)
		}
		snap.RecentCommands = append(snap.RecentCommands, cmd)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate commands: %w", err)
	}

	evRows, err := db.conn.Query(
		`SELECT event_type, payload, created_at
		 FROM events
		 WHERE project_id = ?
		 ORDER BY created_at DESC LIMIT 50`,
		projectID,
	)
	if err != nil {
		return nil, fmt.Errorf("query events: %w", err)
	}
	defer evRows.Close()

	for evRows.Next() {
		var ev SnapshotEvent
		var payload string
		if err := evRows.Scan(&ev.Type, &payload, &ev.Timestamp); err != nil {
			return nil, fmt.Errorf("scan event: %w", err)
		}
		ev.Message = payload
		snap.RecentEvents = append(snap.RecentEvents, ev)
	}
	if err := evRows.Err(); err != nil {
		return nil, fmt.Errorf("iterate events: %w", err)
	}

	return snap, nil
}

func HasSnapshot(db *DB, projectID string) (bool, error) {
	var exists int
	err := db.conn.QueryRow(
		`SELECT 1 FROM workspace_snapshots WHERE project_id = ? LIMIT 1`,
		projectID,
	).Scan(&exists)
	if err == sql.ErrNoRows {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return true, nil
}

func ClearSnapshots(db *DB, projectID string) error {
	_, err := db.conn.Exec(
		`DELETE FROM workspace_snapshots WHERE project_id = ?`,
		projectID,
	)
	return err
}
