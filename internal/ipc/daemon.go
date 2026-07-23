package ipc

import (
	"fmt"
	"log"
	"time"

	"github.com/termforge/forge/internal/docker"
	"github.com/termforge/forge/internal/event"
	"github.com/termforge/forge/internal/knowledge"
	"github.com/termforge/forge/internal/session"
	"github.com/termforge/forge/internal/storage"
	"github.com/termforge/forge/internal/watcher"
)

type Daemon struct {
	db         *storage.DB
	projectID  string
	projectDir string

	sessMgr *session.Manager
	events  *event.Bus
	watcher *watcher.Watcher
	docker  *docker.Service
	server  *Server
}

func NewDaemon(db *storage.DB, projectID, projectDir string) *Daemon {
	d := &Daemon{
		db:         db,
		projectID:  projectID,
		projectDir: projectDir,
		sessMgr:    session.NewManager(),
		events:     event.NewBus(),
	}

	d.events.Subscribe(func(e event.Event) {
		db.InsertEvent(e)
	})

	d.docker = docker.NewService()

	return d
}

func (d *Daemon) Start() error {
	w, err := watcher.New(d.projectDir)
	if err != nil {
		log.Printf("[daemon] file watcher failed: %v", err)
	} else {
		d.watcher = w
		go d.watchFiles()
	}

	d.server = NewServer(d)
	if err := d.server.Start(); err != nil {
		return fmt.Errorf("ipc server: %w", err)
	}

	log.Printf("[daemon] started, project=%s dir=%s port=%d", d.projectID, d.projectDir, d.server.Port())
	return nil
}

func (d *Daemon) Port() int {
	return d.server.Port()
}

func (d *Daemon) Stop() {
	d.sessMgr.Close(d.projectID)

	if d.watcher != nil {
		d.watcher.Stop()
	}
	if d.server != nil {
		d.server.Close()
	}
}

func (d *Daemon) HandleRequest(client *Client, req *Request) {
	var resp Response
	resp.ID = req.ID

	switch req.Method {
	case MethodPing:
		resp.Payload = EncodePayload(map[string]string{"status": "ok"})

	case MethodSessionCreate:
		resp = d.handleSessionCreate(req)
	case MethodSessionWrite:
		resp = d.handleSessionWrite(req)
	case MethodSessionResize:
		resp = d.handleSessionResize(req)
	case MethodSessionClose:
		resp = d.handleSessionClose(req)
	case MethodSessionList:
		resp = d.handleSessionList(req)

	case MethodTaskList:
		resp = d.handleTaskList(req)
	case MethodTaskCreate:
		resp = d.handleTaskCreate(req)
	case MethodTaskActivate:
		resp = d.handleTaskActivate(req)
	case MethodTaskComplete:
		resp = d.handleTaskComplete(req)

	case MethodProjectOpen:
		resp = d.handleProjectOpen(req)
	case MethodProjectCurrent:
		resp = d.handleProjectCurrent(req)

	case MethodEventList:
		resp = d.handleEventList(req)
	case MethodEventSubscribe:
		resp = d.handleEventSubscribe(req)

	case MethodKnowledgeList:
		resp = d.handleKnowledgeList(req)
	case MethodKnowledgeCreate:
		resp = d.handleKnowledgeCreate(req)
	case MethodKnowledgeUpdate:
		resp = d.handleKnowledgeUpdate(req)
	case MethodKnowledgeDelete:
		resp = d.handleKnowledgeDelete(req)
	case MethodKnowledgeGet:
		resp = d.handleKnowledgeGet(req)

	case MethodDockerStatus:
		resp = d.handleDockerStatus(req)
	case MethodDockerRefresh:
		resp = d.handleDockerRefresh(req)
	case MethodDockerStart:
		resp = d.handleDockerStart(req)
	case MethodDockerStop:
		resp = d.handleDockerStop(req)

	case MethodSnapshotSave:
		resp = d.handleSnapshotSave(req)
	case MethodSnapshotLoad:
		resp = d.handleSnapshotLoad(req)

	default:
		resp.Error = fmt.Sprintf("unknown method: %s", req.Method)
	}

	client.Send(&resp)
}

func (d *Daemon) watchFiles() {
	if d.watcher == nil {
		return
	}
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()

	for range ticker.C {
		changes := d.watcher.RecentChanges(10)
		for _, c := range changes {
			d.events.Emit(event.MakeEvent(event.EventFileChanged, d.projectID, map[string]string{
				"path":   c.Path,
				"change": c.Type.String(),
			}))
		}
	}
}

// Session handlers

type SessionCreateRequest struct {
	Name string `json:"name"`
	CWD  string `json:"cwd"`
}

type SessionCreateResponse struct {
	ID string `json:"id"`
}

func (d *Daemon) handleSessionCreate(req *Request) Response {
	var p SessionCreateRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	sess, err := d.sessMgr.Create(d.projectID, "", p.Name, "", p.CWD)
	if err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	d.events.Emit(event.MakeEvent(event.EventSessionStarted, d.projectID, map[string]string{
		"name":       p.Name,
		"session_id": sess.ID,
	}))

	// Start reading output from this session
	d.startOutputReader(sess.ID)

	return Response{
		ID:      req.ID,
		Payload: EncodePayload(SessionCreateResponse{ID: sess.ID}),
	}
}

func (d *Daemon) startOutputReader(sessionID string) {
	go func() {
		for {
			sess, ok := d.sessMgr.Get(sessionID)
			if !ok {
				return
			}

			buf := make([]byte, 8192)
			n, err := sess.Pty.Read(buf)
			if err != nil {
				return
			}

			data := make([]byte, n)
			copy(data, buf[:n])

			d.server.Broadcast(&Event{
				Type:    "session.output",
				Payload: EncodePayload(map[string]interface{}{
					"session_id": sessionID,
					"data":       string(data),
				}),
			})
		}
	}()
}

type SessionWriteRequest struct {
	SessionID string `json:"session_id"`
	Data      string `json:"data"`
}

func (d *Daemon) handleSessionWrite(req *Request) Response {
	var p SessionWriteRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	if err := d.sessMgr.WriteBytes(p.SessionID, []byte(p.Data)); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

type SessionResizeRequest struct {
	SessionID string `json:"session_id"`
	Width     int    `json:"width"`
	Height    int    `json:"height"`
}

func (d *Daemon) handleSessionResize(req *Request) Response {
	var p SessionResizeRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	if err := d.sessMgr.Resize(p.SessionID, p.Width, p.Height); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

type SessionCloseRequest struct {
	SessionID string `json:"session_id"`
}

func (d *Daemon) handleSessionClose(req *Request) Response {
	var p SessionCloseRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	if err := d.sessMgr.Close(p.SessionID); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

type SessionInfo struct {
	ID    string `json:"id"`
	Name  string `json:"name"`
	CWD   string `json:"cwd"`
}

func (d *Daemon) handleSessionList(req *Request) Response {
	sessions := d.sessMgr.List()

	var list []SessionInfo
	for _, s := range sessions {
		list = append(list, SessionInfo{
			ID:   s.ID,
			Name: s.Name,
			CWD:  s.CWD,
		})
	}

	return Response{ID: req.ID, Payload: EncodePayload(list)}
}

// Task handlers

type TaskInfo struct {
	ID     string `json:"id"`
	Title  string `json:"title"`
	Status string `json:"status"`
}

func (d *Daemon) handleTaskList(req *Request) Response {
	tasks, err := d.db.GetTasksByProject(d.projectID)
	if err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	var list []TaskInfo
	for _, t := range tasks {
		list = append(list, TaskInfo{
			ID:     t.ID,
			Title:  t.Title,
			Status: t.Status,
		})
	}

	return Response{ID: req.ID, Payload: EncodePayload(list)}
}

type TaskCreateRequest struct {
	Title string `json:"title"`
}

func (d *Daemon) handleTaskCreate(req *Request) Response {
	var p TaskCreateRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	id := generateID()
	if err := d.db.CreateTask(id, d.projectID, p.Title); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	d.events.Emit(event.MakeEvent(event.EventTaskCreated, d.projectID, map[string]string{
		"task_id": id,
		"title":   p.Title,
	}))

	return Response{ID: req.ID, Payload: EncodePayload(TaskInfo{ID: id, Title: p.Title, Status: "backlog"})}
}

type TaskActivateRequest struct {
	TaskID string `json:"task_id"`
}

func (d *Daemon) handleTaskActivate(req *Request) Response {
	var p TaskActivateRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	// Deactivate current active task
	tasks, _ := d.db.GetTasksByProject(d.projectID)
	for _, t := range tasks {
		if t.Status == "active" {
			d.db.SetTaskStatus(t.ID, "backlog")
		}
	}

	if err := d.db.SetTaskStatus(p.TaskID, "active"); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	d.events.Emit(event.MakeEvent(event.EventTaskActivated, d.projectID, map[string]string{
		"task_id": p.TaskID,
	}))

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

type TaskCompleteRequest struct {
	TaskID string `json:"task_id"`
}

func (d *Daemon) handleTaskComplete(req *Request) Response {
	var p TaskCompleteRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	if err := d.db.SetTaskStatus(p.TaskID, "completed"); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	d.events.Emit(event.MakeEvent(event.EventTaskCompleted, d.projectID, map[string]string{
		"task_id": p.TaskID,
	}))

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

// Project handlers

type ProjectInfo struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	Dir  string `json:"dir"`
}

func (d *Daemon) handleProjectOpen(req *Request) Response {
	return Response{
		ID:      req.ID,
		Payload: EncodePayload(ProjectInfo{ID: d.projectID, Name: d.projectID, Dir: d.projectDir}),
	}
}

func (d *Daemon) handleProjectCurrent(req *Request) Response {
	return Response{
		ID:      req.ID,
		Payload: EncodePayload(ProjectInfo{ID: d.projectID, Name: d.projectID, Dir: d.projectDir}),
	}
}

// Event handlers

func (d *Daemon) handleEventList(req *Request) Response {
	events, err := d.db.GetRecentEvents(d.projectID, 100)
	if err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	type EventInfo struct {
		ID        string `json:"id"`
		Type      string `json:"type"`
		TaskID    string `json:"task_id,omitempty"`
		SessionID string `json:"session_id,omitempty"`
		Payload   string `json:"payload"`
		CreatedAt string `json:"created_at"`
	}

	var list []EventInfo
	for _, e := range events {
		list = append(list, EventInfo{
			ID:        e.ID,
			Type:      string(e.Type),
			TaskID:    e.TaskID,
			SessionID: e.SessionID,
			Payload:   string(e.Payload),
			CreatedAt: e.CreatedAt.Format("2006-01-02T15:04:05Z"),
		})
	}

	return Response{ID: req.ID, Payload: EncodePayload(list)}
}

func (d *Daemon) handleEventSubscribe(req *Request) Response {
	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

// Knowledge handlers

type KnowledgeNodeInfo struct {
	ID       string `json:"id"`
	Title    string `json:"title"`
	Content  string `json:"content"`
	NodeType string `json:"node_type"`
}

func (d *Daemon) handleKnowledgeList(req *Request) Response {
	nodes, err := knowledge.ListNodes(d.db, d.projectID)
	if err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	var list []KnowledgeNodeInfo
	for _, n := range nodes {
		list = append(list, KnowledgeNodeInfo{
			ID:       n.ID,
			Title:    n.Title,
			Content:  n.Content,
			NodeType: string(n.NodeType),
		})
	}

	return Response{ID: req.ID, Payload: EncodePayload(list)}
}

type KnowledgeCreateRequest struct {
	Title    string `json:"title"`
	Content  string `json:"content"`
	NodeType string `json:"node_type"`
}

func (d *Daemon) handleKnowledgeCreate(req *Request) Response {
	var p KnowledgeCreateRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	node, err := knowledge.CreateNode(d.db, d.projectID, p.Title, p.Content, knowledge.NodeType(p.NodeType))
	if err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	return Response{ID: req.ID, Payload: EncodePayload(KnowledgeNodeInfo{
		ID:       node.ID,
		Title:    node.Title,
		Content:  node.Content,
		NodeType: string(node.NodeType),
	})}
}

type KnowledgeUpdateRequest struct {
	ID      string `json:"id"`
	Title   string `json:"title"`
	Content string `json:"content"`
}

func (d *Daemon) handleKnowledgeUpdate(req *Request) Response {
	var p KnowledgeUpdateRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	if err := knowledge.UpdateNode(d.db, p.ID, p.Title, p.Content); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

type KnowledgeDeleteRequest struct {
	ID string `json:"id"`
}

func (d *Daemon) handleKnowledgeDelete(req *Request) Response {
	var p KnowledgeDeleteRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	if err := knowledge.DeleteNode(d.db, p.ID); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

type KnowledgeGetRequest struct {
	ID string `json:"id"`
}

func (d *Daemon) handleKnowledgeGet(req *Request) Response {
	var p KnowledgeGetRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	node, err := knowledge.GetNode(d.db, p.ID)
	if err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}
	if node == nil {
		return Response{ID: req.ID, Error: "not found"}
	}

	return Response{ID: req.ID, Payload: EncodePayload(KnowledgeNodeInfo{
		ID:       node.ID,
		Title:    node.Title,
		Content:  node.Content,
		NodeType: string(node.NodeType),
	})}
}

// Docker handlers

func (d *Daemon) handleDockerStatus(req *Request) Response {
	status := d.docker.GetStatus()
	return Response{ID: req.ID, Payload: EncodePayload(status)}
}

func (d *Daemon) handleDockerRefresh(req *Request) Response {
	status := d.docker.GetStatus()
	return Response{ID: req.ID, Payload: EncodePayload(status)}
}

type DockerContainerRequest struct {
	ID string `json:"id"`
}

func (d *Daemon) handleDockerStart(req *Request) Response {
	var p DockerContainerRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	if err := d.docker.StartContainer(p.ID); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

func (d *Daemon) handleDockerStop(req *Request) Response {
	var p DockerContainerRequest
	if err := DecodePayload(req.Payload, &p); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	if err := d.docker.StopContainer(p.ID); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

// Snapshot handlers

func (d *Daemon) handleSnapshotSave(req *Request) Response {
	snap, err := storage.BuildSnapshot(d.db, d.projectID)
	if err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	if err := storage.SaveSnapshot(d.db, snap); err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}

	return Response{ID: req.ID, Payload: EncodePayload(map[string]bool{"ok": true})}
}

func (d *Daemon) handleSnapshotLoad(req *Request) Response {
	snap, err := storage.GetLatestSnapshot(d.db, d.projectID)
	if err != nil {
		return Response{ID: req.ID, Error: err.Error()}
	}
	if snap == nil {
		return Response{ID: req.ID, Payload: EncodePayload(nil)}
	}

	return Response{ID: req.ID, Payload: EncodePayload(snap)}
}

// Helpers

func generateID() string {
	return fmt.Sprintf("%d", time.Now().UnixNano())
}
