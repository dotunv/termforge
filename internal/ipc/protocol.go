package ipc

import (
	"encoding/json"
	"fmt"
	"io"
)

type Request struct {
	ID      string          `json:"id"`
	Method  string          `json:"method"`
	Payload json.RawMessage `json:"payload,omitempty"`
}

type Response struct {
	ID      string          `json:"id"`
	Error   string          `json:"error,omitempty"`
	Payload json.RawMessage `json:"payload,omitempty"`
}

type Event struct {
	ID        string          `json:"id"`
	Type      string          `json:"type"`
	ProjectID string          `json:"project_id"`
	Payload   json.RawMessage `json:"payload"`
}

type SubscribeRequest struct {
	ProjectID string `json:"project_id"`
}

type SubscribeResponse struct {
	OK bool `json:"ok"`
}

// Session methods
const (
	MethodSessionCreate  = "session.create"
	MethodSessionWrite   = "session.write"
	MethodSessionResize  = "session.resize"
	MethodSessionClose   = "session.close"
	MethodSessionList    = "session.list"
	MethodSessionOutput  = "session.output"
)

// Task methods
const (
	MethodTaskList    = "task.list"
	MethodTaskCreate  = "task.create"
	MethodTaskActivate = "task.activate"
	MethodTaskComplete = "task.complete"
	MethodTaskDelete  = "task.delete"
)

// Project methods
const (
	MethodProjectOpen    = "project.open"
	MethodProjectCurrent = "project.current"
)

// Event methods
const (
	MethodEventList     = "event.list"
	MethodEventSubscribe = "event.subscribe"
)

// Knowledge methods
const (
	MethodKnowledgeList   = "knowledge.list"
	MethodKnowledgeCreate = "knowledge.create"
	MethodKnowledgeUpdate = "knowledge.update"
	MethodKnowledgeDelete = "knowledge.delete"
	MethodKnowledgeGet    = "knowledge.get"
)

// Docker methods
const (
	MethodDockerStatus    = "docker.status"
	MethodDockerRefresh   = "docker.refresh"
	MethodDockerStart     = "docker.start"
	MethodDockerStop      = "docker.stop"
)

// Snapshot methods
const (
	MethodSnapshotSave   = "snapshot.save"
	MethodSnapshotLoad   = "snapshot.load"
)

// System methods
const (
	MethodPing = "ping"
)

// ReadMessage reads a length-prefixed JSON message from a stream.
// Format: 4 bytes big-endian length + JSON payload
func ReadMessage(r io.Reader) (*Request, error) {
	var length uint32
	if err := readUint32(r, &length); err != nil {
		return nil, err
	}

	if length > 1024*1024 {
		return nil, fmt.Errorf("message too large: %d bytes", length)
	}

	buf := make([]byte, length)
	if _, err := io.ReadFull(r, buf); err != nil {
		return nil, err
	}

	var req Request
	if err := json.Unmarshal(buf, &req); err != nil {
		return nil, fmt.Errorf("unmarshal request: %w", err)
	}

	return &req, nil
}

// WriteMessage writes a length-prefixed JSON message to a stream.
func WriteMessage(w io.Writer, msg interface{}) error {
	data, err := json.Marshal(msg)
	if err != nil {
		return fmt.Errorf("marshal message: %w", err)
	}

	if err := writeUint32(w, uint32(len(data))); err != nil {
		return err
	}

	_, err = w.Write(data)
	return err
}

// ReadEvent reads a length-prefixed event from a stream.
func ReadEvent(r io.Reader) (*Event, error) {
	var length uint32
	if err := readUint32(r, &length); err != nil {
		return nil, err
	}

	if length > 1024*1024 {
		return nil, fmt.Errorf("event too large: %d bytes", length)
	}

	buf := make([]byte, length)
	if _, err := io.ReadFull(r, buf); err != nil {
		return nil, err
	}

	var evt Event
	if err := json.Unmarshal(buf, &evt); err != nil {
		return nil, fmt.Errorf("unmarshal event: %w", err)
	}

	return &evt, nil
}

// WriteEvent writes a length-prefixed event to a stream.
func WriteEvent(w io.Writer, evt *Event) error {
	return WriteMessage(w, evt)
}

func readUint32(r io.Reader, v *uint32) error {
	buf := make([]byte, 4)
	if _, err := io.ReadFull(r, buf); err != nil {
		return err
	}
	*v = uint32(buf[0])<<24 | uint32(buf[1])<<16 | uint32(buf[2])<<8 | uint32(buf[3])
	return nil
}

func writeUint32(w io.Writer, v uint32) error {
	buf := []byte{
		byte(v >> 24),
		byte(v >> 16),
		byte(v >> 8),
		byte(v),
	}
	_, err := w.Write(buf)
	return err
}

func EncodePayload(v interface{}) json.RawMessage {
	data, _ := json.Marshal(v)
	return data
}

func DecodePayload(data json.RawMessage, v interface{}) error {
	return json.Unmarshal(data, v)
}
