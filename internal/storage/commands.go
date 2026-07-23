package storage

import (
	"fmt"
	"time"

	"github.com/termforge/forge/internal/event"
)

type CommandRow struct {
	ID           string
	SessionID    string
	TaskID       string
	TaskTitle    string
	InputCommand string
	ExitCode     int
	DurationMs   int
	StartedAt    time.Time
}

type SessionRow struct {
	ID        string
	ProjectID string
	Name      string
	CWD       string
	IsActive  bool
	CreatedAt time.Time
}

func LogCommand(db *DB, sessionID, taskID, input string, durationMs int, exitCode int) error {
	id := event.MakeEvent("", "", nil).ID
	_, err := db.conn.Exec(
		`INSERT INTO commands (id, session_id, task_id, input_command, exit_code, execution_duration_ms, started_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?)`,
		id, sessionID, taskID, input, exitCode, durationMs, time.Now(),
	)
	if err != nil {
		return fmt.Errorf("log command: %w", err)
	}
	return nil
}

func GetRecentCommands(db *DB, projectID string, limit int) ([]CommandRow, error) {
	if limit <= 0 {
		limit = 50
	}
	rows, err := db.conn.Query(
		`SELECT c.id, c.session_id, COALESCE(c.task_id,''), COALESCE(t.title,''),
		        c.input_command, COALESCE(c.exit_code,0), COALESCE(c.execution_duration_ms,0), c.started_at
		 FROM commands c
		 LEFT JOIN tasks t ON c.task_id = t.id
		 JOIN sessions s ON c.session_id = s.id
		 WHERE s.project_id = ?
		 ORDER BY c.started_at DESC LIMIT ?`,
		projectID, limit,
	)
	if err != nil {
		return nil, fmt.Errorf("get recent commands: %w", err)
	}
	defer rows.Close()

	var commands []CommandRow
	for rows.Next() {
		var cmd CommandRow
		if err := rows.Scan(&cmd.ID, &cmd.SessionID, &cmd.TaskID, &cmd.TaskTitle,
			&cmd.InputCommand, &cmd.ExitCode, &cmd.DurationMs, &cmd.StartedAt); err != nil {
			return nil, fmt.Errorf("scan command row: %w", err)
		}
		commands = append(commands, cmd)
	}
	return commands, nil
}

func UpdateSessionCWD(db *DB, sessionID, cwd string) error {
	_, err := db.conn.Exec(
		`UPDATE sessions SET current_working_dir = ? WHERE id = ?`,
		cwd, sessionID,
	)
	if err != nil {
		return fmt.Errorf("update session cwd: %w", err)
	}
	return nil
}

func GetActiveSessions(db *DB, projectID string) ([]SessionRow, error) {
	rows, err := db.conn.Query(
		`SELECT id, project_id, name, current_working_dir, is_active, created_at
		 FROM sessions
		 WHERE project_id = ? AND is_active = 1
		 ORDER BY created_at DESC`,
		projectID,
	)
	if err != nil {
		return nil, fmt.Errorf("get active sessions: %w", err)
	}
	defer rows.Close()

	var sessions []SessionRow
	for rows.Next() {
		var s SessionRow
		var isActive int
		if err := rows.Scan(&s.ID, &s.ProjectID, &s.Name, &s.CWD, &isActive, &s.CreatedAt); err != nil {
			return nil, fmt.Errorf("scan session row: %w", err)
		}
		s.IsActive = isActive == 1
		sessions = append(sessions, s)
	}
	return sessions, nil
}

func MarkSessionInactive(db *DB, sessionID string) error {
	_, err := db.conn.Exec(
		`UPDATE sessions SET is_active = 0 WHERE id = ?`,
		sessionID,
	)
	if err != nil {
		return fmt.Errorf("mark session inactive: %w", err)
	}
	return nil
}
