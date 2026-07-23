package event

import (
	"encoding/json"
	"log"
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
	EventFileChanged     EventType = "file.changed"
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
	mu       sync.Mutex
	handlers map[EventType][]Handler
}

func NewBus() *Bus {
	return &Bus{
		handlers: make(map[EventType][]Handler),
	}
}

func (b *Bus) Subscribe(fn Handler) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.handlers["*"] = append(b.handlers["*"], fn)
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

	b.mu.Lock()
	selected := make([]Handler, 0)
	if specific, ok := b.handlers[e.Type]; ok {
		selected = append(selected, specific...)
	}
	if all, ok := b.handlers["*"]; ok {
		selected = append(selected, all...)
	}
	b.mu.Unlock()

	for _, fn := range selected {
		func() {
			defer func() {
				if r := recover(); r != nil {
					log.Printf("[event] handler panic for %s: %v", e.Type, r)
				}
			}()
			fn(e)
		}()
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
