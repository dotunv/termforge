package ws

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"sync"

	"github.com/gorilla/websocket"
	"github.com/rs/zerolog"
	"github.com/termforge/forge/internal/event"
	"github.com/termforge/forge/internal/session"
)

var upgrader = websocket.Upgrader{
	CheckOrigin: func(r *http.Request) bool { return true },
}

type Server struct {
	addr   string
	log    zerolog.Logger
	sess   *session.Manager
	events *event.Bus
	conns  map[*websocket.Conn]bool
	mu     sync.RWMutex
}

type Message struct {
	Type    string          `json:"type"`
	Payload json.RawMessage `json:"payload"`
}

func New(addr string, log zerolog.Logger, sess *session.Manager, bus *event.Bus) *Server {
	return &Server{
		addr:   addr,
		log:    log,
		sess:   sess,
		events: bus,
		conns:  make(map[*websocket.Conn]bool),
	}
}

func (s *Server) Run(ctx context.Context) error {
	mux := http.NewServeMux()
	mux.HandleFunc("/ws", s.handleWS)
	mux.HandleFunc("/api/health", s.handleHealth)

	srv := &http.Server{
		Addr:    s.addr,
		Handler: mux,
	}

	go func() {
		<-ctx.Done()
		srv.Close()
	}()

	s.log.Info().Str("addr", s.addr).Msg("websocket server listening")
	if err := srv.ListenAndServe(); err != http.ErrServerClosed {
		return err
	}
	return nil
}

func (s *Server) handleHealth(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.Write([]byte(`{"status":"ok"}`))
}

func (s *Server) handleWS(w http.ResponseWriter, r *http.Request) {
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		s.log.Error().Err(err).Msg("websocket upgrade failed")
		return
	}

	s.mu.Lock()
	s.conns[conn] = true
	s.mu.Unlock()

	defer func() {
		s.mu.Lock()
		delete(s.conns, conn)
		s.mu.Unlock()
		conn.Close()
	}()

	s.log.Info().Str("remote", r.RemoteAddr).Msg("client connected")

	for {
		_, msg, err := conn.ReadMessage()
		if err != nil {
			s.log.Debug().Err(err).Msg("client disconnected")
			return
		}

		var m Message
		if err := json.Unmarshal(msg, &m); err != nil {
			s.log.Warn().Err(err).Msg("invalid message")
			continue
		}

		s.handleMessage(conn, m)
	}
}

func (s *Server) handleMessage(conn *websocket.Conn, m Message) {
	switch m.Type {
	case "session.create":
		s.handleSessionCreate(conn, m)
	case "session.input":
		s.handleSessionInput(conn, m)
	case "session.resize":
		s.handleSessionResize(conn, m)
	case "task.activate":
		s.handleTaskActivate(conn, m)
	default:
		s.log.Warn().Str("type", m.Type).Msg("unknown message type")
	}
}

func (s *Server) handleSessionCreate(conn *websocket.Conn, m Message) {
	var payload struct {
		ProjectID string `json:"project_id"`
		TaskID    string `json:"task_id"`
		Name      string `json:"name"`
		Shell     string `json:"shell"`
		CWD       string `json:"cwd"`
		Width     int    `json:"width"`
		Height    int    `json:"height"`
	}
	if err := json.Unmarshal(m.Payload, &payload); err != nil {
		s.sendError(conn, "invalid payload")
		return
	}

	sess, err := s.sess.Create(payload.ProjectID, payload.TaskID, payload.Name, payload.Shell, payload.CWD)
	if err != nil {
		s.sendError(conn, err.Error())
		return
	}

	s.events.Emit(event.Event{
		Type:      event.EventSessionStarted,
		ProjectID: payload.ProjectID,
		SessionID: sess.ID,
		TaskID:    payload.TaskID,
		Payload:   []byte(`{"name":"` + payload.Name + `"}`),
	})

	resp, _ := json.Marshal(map[string]string{
		"session_id": sess.ID,
	})
	s.send(conn, Message{Type: "session.created", Payload: resp})

	go s.streamPTY(conn, sess.ID)
}

func (s *Server) handleSessionInput(conn *websocket.Conn, m Message) {
	var payload struct {
		SessionID string `json:"session_id"`
		Data      string `json:"data"`
	}
	if err := json.Unmarshal(m.Payload, &payload); err != nil {
		s.sendError(conn, "invalid payload")
		return
	}

	if err := s.sess.WriteFrom(payload.SessionID, strings.NewReader(payload.Data)); err != nil {
		s.sendError(conn, err.Error())
	}
}

func (s *Server) handleSessionResize(conn *websocket.Conn, m Message) {
	var payload struct {
		SessionID string `json:"session_id"`
		Width     int    `json:"width"`
		Height    int    `json:"height"`
	}
	if err := json.Unmarshal(m.Payload, &payload); err != nil {
		s.sendError(conn, "invalid payload")
		return
	}

	if err := s.sess.Resize(payload.SessionID, payload.Width, payload.Height); err != nil {
		s.sendError(conn, err.Error())
	}
}

func (s *Server) handleTaskActivate(conn *websocket.Conn, m Message) {
	var payload struct {
		TaskID    string `json:"task_id"`
		ProjectID string `json:"project_id"`
	}
	if err := json.Unmarshal(m.Payload, &payload); err != nil {
		s.sendError(conn, "invalid payload")
		return
	}

	s.events.Emit(event.Event{
		Type:      event.EventTaskActivated,
		ProjectID: payload.ProjectID,
		TaskID:    payload.TaskID,
		Payload:   []byte(`{}`),
	})

	s.send(conn, Message{Type: "task.activated", Payload: m.Payload})
}

func (s *Server) streamPTY(conn *websocket.Conn, sessionID string) {
	sess, ok := s.sess.Get(sessionID)
	if !ok {
		s.log.Error().Str("session_id", sessionID).Msg("session not found for streaming")
		return
	}

	for {
		var buf [4096]byte
		n, err := sess.Pty.Read(buf[:])
		if err != nil {
			return
		}

		payload, _ := json.Marshal(map[string]interface{}{
			"session_id": sessionID,
			"data":       string(buf[:n]),
		})
		s.send(conn, Message{Type: "session.output", Payload: payload})
	}
}

func (s *Server) send(conn *websocket.Conn, m Message) {
	data, _ := json.Marshal(m)
	conn.WriteMessage(websocket.TextMessage, data)
}

func (s *Server) sendError(conn *websocket.Conn, msg string) {
	payload, _ := json.Marshal(map[string]string{"error": msg})
	s.send(conn, Message{Type: "error", Payload: payload})
}

func (s *Server) Broadcast(m Message) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for conn := range s.conns {
		s.send(conn, m)
	}
}
