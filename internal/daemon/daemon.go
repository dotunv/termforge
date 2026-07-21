package daemon

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"sync"

	"github.com/termforge/forge/internal/config"
	"github.com/termforge/forge/internal/event"
	"github.com/termforge/forge/internal/session"
	"github.com/termforge/forge/internal/storage"
	"github.com/termforge/forge/internal/ws"
	"github.com/rs/zerolog"
)

type Daemon struct {
	cfg      *config.Config
	db       *storage.DB
	log      zerolog.Logger
	events   *event.Bus
	sessions *session.Manager
	ws       *ws.Server
	mu       sync.Mutex
}

type Opts struct {
	Config *config.Config
	DB     *storage.DB
	Logger zerolog.Logger
}

func New(opts Opts) (*Daemon, error) {
	bus := event.NewBus()
	sessMgr := session.NewManager()
	wsSrv := ws.New(opts.Config.ListenAddr, opts.Logger, sessMgr, bus)

	d := &Daemon{
		cfg:      opts.Config,
		db:       opts.DB,
		log:      opts.Logger,
		events:   bus,
		sessions: sessMgr,
		ws:       wsSrv,
	}

	d.wireEvents()

	return d, nil
}

func (d *Daemon) wireEvents() {
	d.events.Subscribe(func(e event.Event) {
		if err := d.db.InsertEvent(e); err != nil {
			d.log.Error().Err(err).Str("event_type", string(e.Type)).Msg("failed to persist event")
		}
	})
}

func (d *Daemon) Run(ctx context.Context) error {
	wd, _ := os.Getwd()
	d.db.UpsertProject("default", "default", wd)

	d.events.Emit(event.MakeEvent(event.EventProjectOpened, "default", map[string]string{
		"root_directory": wd,
	}))

	errCh := make(chan error, 1)

	go func() {
		errCh <- d.ws.Run(ctx)
	}()

	select {
	case err := <-errCh:
		return fmt.Errorf("ws server error: %w", err)
	case <-ctx.Done():
		d.log.Info().Msg("shutting down daemon")
		d.saveSnapshot()
		return nil
	}
}

func (d *Daemon) saveSnapshot() {
	d.mu.Lock()
	defer d.mu.Unlock()

	sessions := d.sessions.List()
	type snapSession struct {
		ID    string `json:"id"`
		Name  string `json:"name"`
		CWD   string `json:"cwd"`
		Shell string `json:"shell"`
	}
	var snaps []snapSession
	for _, s := range sessions {
		snaps = append(snaps, snapSession{ID: s.ID, Name: s.Name, CWD: s.CWD})
	}

	data, _ := json.Marshal(map[string]interface{}{
		"sessions": snaps,
	})

	d.db.Conn().Exec(
		`INSERT INTO workspace_snapshots (id, project_id, snapshot_data) VALUES (?, ?, ?)`,
		event.MakeEvent("", "", nil).ID, "default", string(data),
	)

	d.log.Info().Int("sessions", len(snaps)).Msg("workspace snapshot saved")
}

func (d *Daemon) GetEventBus() *event.Bus {
	return d.events
}
