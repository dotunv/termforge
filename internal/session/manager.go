package session

import (
	"fmt"
	"io"
	"os"
	"sync"

	"github.com/google/uuid"
)

type Manager struct {
	sessions map[string]*Session
	mu       sync.RWMutex
}

type Session struct {
	ID        string
	ProjectID string
	TaskID    string
	Name      string
	Pty       io.ReadWriteCloser
	CWD       string
}

func NewManager() *Manager {
	return &Manager{
		sessions: make(map[string]*Session),
	}
}

func (m *Manager) Create(projectID, taskID, name, shell, cwd string) (*Session, error) {
	if cwd == "" {
		cwd, _ = os.Getwd()
	}

	pty, err := startPty(cwd)
	if err != nil {
		return nil, fmt.Errorf("start pty: %w", err)
	}

	sess := &Session{
		ID:        uuid.New().String(),
		ProjectID: projectID,
		TaskID:    taskID,
		Name:      name,
		Pty:       pty,
		CWD:       cwd,
	}

	m.mu.Lock()
	m.sessions[sess.ID] = sess
	m.mu.Unlock()

	return sess, nil
}

func (m *Manager) Get(id string) (*Session, bool) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	s, ok := m.sessions[id]
	return s, ok
}

func (m *Manager) List() []*Session {
	m.mu.RLock()
	defer m.mu.RUnlock()
	var sessions []*Session
	for _, s := range m.sessions {
		sessions = append(sessions, s)
	}
	return sessions
}

func (m *Manager) WriteBytes(id string, data []byte) error {
	s, ok := m.Get(id)
	if !ok {
		return fmt.Errorf("session %s not found", id)
	}
	_, err := s.Pty.Write(data)
	return err
}

func (m *Manager) Resize(id string, width, height int) error {
	s, ok := m.Get(id)
	if !ok {
		return fmt.Errorf("session %s not found", id)
	}
	return resizePty(s, width, height)
}

func (m *Manager) Close(id string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	s, ok := m.sessions[id]
	if !ok {
		return fmt.Errorf("session %s not found", id)
	}

	s.Pty.Close()
	delete(m.sessions, id)
	return nil
}
