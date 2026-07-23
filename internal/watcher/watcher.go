package watcher

import (
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
)

type ChangeType int

const (
	ChangeCreated ChangeType = iota
	ChangeModified
	ChangeDeleted
	ChangeRenamed
)

func (c ChangeType) String() string {
	switch c {
	case ChangeCreated:
		return "created"
	case ChangeModified:
		return "modified"
	case ChangeDeleted:
		return "deleted"
	case ChangeRenamed:
		return "renamed"
	default:
		return "unknown"
	}
}

type FileChange struct {
	Path      string
	OldPath   string
	Type      ChangeType
	Timestamp time.Time
}

type Watcher struct {
	root    string
	watcher *fsnotify.Watcher
	mu      sync.Mutex
	changes []FileChange
	maxBuf  int
	stopCh  chan struct{}
	ignored map[string]bool
}

func New(root string) (*Watcher, error) {
	w, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}

	vw := &Watcher{
		root:    root,
		watcher: w,
		maxBuf:  200,
		stopCh:  make(chan struct{}),
		ignored: defaultIgnored(),
	}

	if err := vw.addRecursive(root); err != nil {
		w.Close()
		return nil, err
	}

	go vw.loop()

	return vw, nil
}

func defaultIgnored() map[string]bool {
	return map[string]bool{
		".git":          true,
		"node_modules":  true,
		".DS_Store":     true,
		"Thumbs.db":     true,
		"__pycache__":   true,
		".pytest_cache": true,
		".mypy_cache":   true,
		"vendor":        true,
		".vagrant":      true,
		"dist":          true,
		"build":         true,
		".next":         true,
		".nuxt":         true,
	}
}

func (w *Watcher) addRecursive(dir string) error {
	return filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil
		}
		if info.IsDir() {
			base := filepath.Base(path)
			if w.ignored[base] {
				return filepath.SkipDir
			}
			return w.watcher.Add(path)
		}
		return nil
	})
}

func (w *Watcher) loop() {
	debounce := make(map[string]*time.Timer)
	var debounceMu sync.Mutex

	for {
		select {
		case <-w.stopCh:
			return
		case event, ok := <-w.watcher.Events:
			if !ok {
				return
			}

			if w.shouldIgnore(event.Name) {
				continue
			}

			path := event.Name

			debounceMu.Lock()
			if t, exists := debounce[path]; exists {
				t.Stop()
			}

			evtType := eventToChangeType(event)
			if evtType == -1 {
				debounceMu.Unlock()
				continue
			}

			debounce[path] = time.AfterFunc(100*time.Millisecond, func() {
				change := FileChange{
					Path:      path,
					Type:      evtType,
					Timestamp: time.Now(),
				}
				w.mu.Lock()
				w.changes = append(w.changes, change)
				if len(w.changes) > w.maxBuf {
					w.changes = w.changes[len(w.changes)-w.maxBuf:]
				}
				w.mu.Unlock()
			})
			debounceMu.Unlock()

		case err, ok := <-w.watcher.Errors:
			if !ok {
				return
			}
			log.Printf("[watcher] error: %v", err)
		}
	}
}

func eventToChangeType(event fsnotify.Event) ChangeType {
	switch {
	case event.Op&fsnotify.Create == fsnotify.Create:
		return ChangeCreated
	case event.Op&fsnotify.Write == fsnotify.Write:
		return ChangeModified
	case event.Op&fsnotify.Remove == fsnotify.Remove:
		return ChangeDeleted
	case event.Op&fsnotify.Rename == fsnotify.Rename:
		return ChangeRenamed
	default:
		return -1
	}
}

func (w *Watcher) shouldIgnore(path string) bool {
	parts := strings.Split(filepath.ToSlash(path), "/")
	for _, part := range parts {
		if w.ignored[part] {
			return true
		}
	}
	return false
}

func (w *Watcher) GetChanges(since time.Time) []FileChange {
	w.mu.Lock()
	defer w.mu.Unlock()

	var result []FileChange
	for _, c := range w.changes {
		if c.Timestamp.After(since) {
			result = append(result, c)
		}
	}
	return result
}

func (w *Watcher) RecentChanges(limit int) []FileChange {
	w.mu.Lock()
	defer w.mu.Unlock()

	if limit <= 0 || limit > len(w.changes) {
		limit = len(w.changes)
	}
	start := len(w.changes) - limit
	if start < 0 {
		start = 0
	}
	result := make([]FileChange, len(w.changes[start:]))
	copy(result, w.changes[start:])
	return result
}

func (w *Watcher) Stop() {
	close(w.stopCh)
	w.watcher.Close()
}
