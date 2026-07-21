package event

import (
	"encoding/json"
	"sync"
	"time"

	"github.com/google/uuid"
)

type EventType string

const (
	EventSessionStarted  EventType = "session.started"
	EventSessionStopped  EventType = "session.stopped"
	EventCommandExecuted EventType = "command.executed"
	EventTaskActivated   EventType = "task.activated"
	EventTaskCreated     EventType = "task.created"
	EventTaskCompleted   EventType = "task.completed"
	EventProjectOpened   EventType = "project.opened"
	EventNoteCreated     EventType = "note.created"
)

type Event struct {
	ID        string          `json:"id"`
	Type      EventType       `json:"type"`
	ProjectID string          `json:"project_id"`
	TaskID    string          `json:"task_id,omitempty"`
	SessionID string          `json:"session_id,omitempty"`
	Payload   json.RawMessage `json:"payload"`
	CreatedAt time.Time       `json:"created_at"`
}

type Handler func(Event)

type Bus struct {
	mu       sync.RWMutex
	handlers map[EventType][]Handler
	all      []Handler
}

func NewBus() *Bus {
	return &Bus{
		handlers: make(map[EventType][]Handler),
	}
}

func (b *Bus) Subscribe(fn Handler) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.all = append(b.all, fn)
}

func (b *Bus) SubscribeType(t EventType, fn Handler) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.handlers[t] = append(b.handlers[t], fn)
}

func (b *Bus) Emit(e Event) {
	if e.ID == "" {
		e.ID = uuid.New().String()
	}
	if e.CreatedAt.IsZero() {
		e.CreatedAt = time.Now()
	}

	b.mu.RLock()
	defer b.mu.RUnlock()

	for _, fn := range b.handlers[e.Type] {
		go fn(e)
	}
	for _, fn := range b.all {
		go fn(e)
	}
}

func MakeEvent(t EventType, projectID string, payload interface{}) Event {
	data, _ := json.Marshal(payload)
	return Event{
		ID:        uuid.New().String(),
		Type:      t,
		ProjectID: projectID,
		Payload:   data,
		CreatedAt: time.Now(),
	}
}
