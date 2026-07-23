package event

import (
	"sync"
	"testing"
	"time"
)

func TestBusSubscribeAndEmit(t *testing.T) {
	bus := NewBus()
	var received []Event
	var mu sync.Mutex

	bus.Subscribe(func(e Event) {
		mu.Lock()
		received = append(received, e)
		mu.Unlock()
	})

	e := MakeEvent(EventSessionStarted, "proj1", map[string]string{"name": "test"})
	bus.Emit(e)

	mu.Lock()
	defer mu.Unlock()
	if len(received) != 1 {
		t.Fatalf("expected 1 event, got %d", len(received))
	}
	if received[0].Type != EventSessionStarted {
		t.Fatalf("expected session.started, got %s", received[0].Type)
	}
	if received[0].ProjectID != "proj1" {
		t.Fatalf("expected proj1, got %s", received[0].ProjectID)
	}
}

func TestBusSubscribeType(t *testing.T) {
	bus := NewBus()
	var sessionEvents []Event
	var commandEvents []Event
	var mu sync.Mutex

	bus.SubscribeType(EventSessionStarted, func(e Event) {
		mu.Lock()
		sessionEvents = append(sessionEvents, e)
		mu.Unlock()
	})

	bus.SubscribeType(EventCommandExecuted, func(e Event) {
		mu.Lock()
		commandEvents = append(commandEvents, e)
		mu.Unlock()
	})

	bus.Emit(MakeEvent(EventSessionStarted, "proj1", nil))
	bus.Emit(MakeEvent(EventCommandExecuted, "proj1", nil))
	bus.Emit(MakeEvent(EventSessionStarted, "proj1", nil))

	mu.Lock()
	defer mu.Unlock()
	if len(sessionEvents) != 2 {
		t.Fatalf("expected 2 session events, got %d", len(sessionEvents))
	}
	if len(commandEvents) != 1 {
		t.Fatalf("expected 1 command event, got %d", len(commandEvents))
	}
}

func TestBusPanicRecovery(t *testing.T) {
	bus := NewBus()
	var recovered bool

	bus.Subscribe(func(e Event) {
		panic("test panic")
	})

	bus.Subscribe(func(e Event) {
		recovered = true
	})

	bus.Emit(MakeEvent(EventSessionStarted, "proj1", nil))

	if !recovered {
		t.Fatal("expected second handler to run after panic recovery")
	}
}

func TestMakeEvent(t *testing.T) {
	e := MakeEvent(EventCommandExecuted, "proj1", map[string]string{"cmd": "ls"})

	if e.ID == "" {
		t.Fatal("expected non-empty ID")
	}
	if e.Type != EventCommandExecuted {
		t.Fatalf("expected command.executed, got %s", e.Type)
	}
	if e.ProjectID != "proj1" {
		t.Fatalf("expected proj1, got %s", e.ProjectID)
	}
	if e.CreatedAt.IsZero() {
		t.Fatal("expected non-zero CreatedAt")
	}
}

func TestBusConcurrentEmit(t *testing.T) {
	bus := NewBus()
	var count int
	var mu sync.Mutex

	bus.Subscribe(func(e Event) {
		mu.Lock()
		count++
		mu.Unlock()
	})

	var wg sync.WaitGroup
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			bus.Emit(MakeEvent(EventSessionStarted, "proj1", nil))
		}()
	}
	wg.Wait()

	mu.Lock()
	defer mu.Unlock()
	if count != 100 {
		t.Fatalf("expected 100 events, got %d", count)
	}
}

func TestEventTypeConstants(t *testing.T) {
	types := []EventType{
		EventSessionStarted,
		EventSessionStopped,
		EventCommandExecuted,
		EventTaskActivated,
		EventTaskCreated,
		EventTaskCompleted,
		EventProjectOpened,
		EventNoteCreated,
		EventFileChanged,
	}

	seen := make(map[EventType]bool)
	for _, et := range types {
		if seen[et] {
			t.Fatalf("duplicate event type: %s", et)
		}
		seen[et] = true
		if string(et) == "" {
			t.Fatal("empty event type string")
		}
	}

	if len(types) != 9 {
		t.Fatalf("expected 9 event types, got %d", len(types))
	}
}

func TestEventTimestamp(t *testing.T) {
	before := time.Now()
	e := MakeEvent(EventSessionStarted, "proj1", nil)
	after := time.Now()

	if e.CreatedAt.Before(before) || e.CreatedAt.After(after) {
		t.Fatalf("timestamp out of range: %v not in [%v, %v]", e.CreatedAt, before, after)
	}
}
