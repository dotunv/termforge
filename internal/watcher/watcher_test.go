package watcher

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestNewWatcher(t *testing.T) {
	dir := t.TempDir()
	w, err := New(dir)
	if err != nil {
		t.Fatalf("New failed: %v", err)
	}
	defer w.Stop()

	if w.root != dir {
		t.Fatalf("expected root %s, got %s", dir, w.root)
	}
}

func TestIgnoreDirectories(t *testing.T) {
	w := &Watcher{
		ignored: defaultIgnored(),
	}

	tests := []struct {
		path    string
		ignored bool
	}{
		{"/project/.git/config", true},
		{"/project/node_modules/pkg/index.js", true},
		{"/project/src/main.go", false},
		{"/project/test.go", false},
		{"/project/.DS_Store", true},
	}

	for _, tt := range tests {
		if got := w.shouldIgnore(tt.path); got != tt.ignored {
			t.Errorf("shouldIgnore(%s) = %v, want %v", tt.path, got, tt.ignored)
		}
	}
}

func TestRecentChangesEmpty(t *testing.T) {
	dir := t.TempDir()
	w, err := New(dir)
	if err != nil {
		t.Fatalf("New failed: %v", err)
	}
	defer w.Stop()

	changes := w.RecentChanges(10)
	if len(changes) != 0 {
		t.Fatalf("expected 0 changes, got %d", len(changes))
	}
}

func TestFileChangeDetection(t *testing.T) {
	dir := t.TempDir()
	w, err := New(dir)
	if err != nil {
		t.Fatalf("New failed: %v", err)
	}
	defer w.Stop()

	// Create a file
	testFile := filepath.Join(dir, "test.txt")
	os.WriteFile(testFile, []byte("hello"), 0644)

	// Wait for debounce
	time.Sleep(200 * time.Millisecond)

	changes := w.RecentChanges(10)
	if len(changes) == 0 {
		t.Fatal("expected at least 1 change")
	}

	found := false
	for _, c := range changes {
		if filepath.Base(c.Path) == "test.txt" && c.Type == ChangeCreated {
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("expected create change for test.txt, got %v", changes)
	}
}

func TestGetChangesSince(t *testing.T) {
	dir := t.TempDir()
	w, err := New(dir)
	if err != nil {
		t.Fatalf("New failed: %v", err)
	}
	defer w.Stop()

	before := time.Now()

	os.WriteFile(filepath.Join(dir, "a.txt"), []byte("a"), 0644)
	time.Sleep(200 * time.Millisecond)

	after := time.Now()
	time.Sleep(100 * time.Millisecond)

	os.WriteFile(filepath.Join(dir, "b.txt"), []byte("b"), 0644)
	time.Sleep(200 * time.Millisecond)

	changes := w.RecentChanges(10)
	if len(changes) < 2 {
		t.Fatalf("expected at least 2 changes, got %d", len(changes))
	}

	_ = before
	_ = after
}

func TestMaxBuffer(t *testing.T) {
	w := &Watcher{
		maxBuf: 5,
	}

	for i := 0; i < 10; i++ {
		w.changes = append(w.changes, FileChange{
			Path:      "file" + string(rune('0'+i)),
			Type:      ChangeModified,
			Timestamp: time.Now(),
		})
	}

	result := w.RecentChanges(10)
	if len(result) > 5 {
		t.Fatalf("expected max 5 changes, got %d", len(result))
	}
}

func TestChangeTypeString(t *testing.T) {
	tests := []struct {
		ct   ChangeType
		want string
	}{
		{ChangeCreated, "created"},
		{ChangeModified, "modified"},
		{ChangeDeleted, "deleted"},
		{ChangeRenamed, "renamed"},
		{ChangeType(99), "unknown"},
	}

	for _, tt := range tests {
		if got := tt.ct.String(); got != tt.want {
			t.Errorf("ChangeType(%d).String() = %s, want %s", tt.ct, got, tt.want)
		}
	}
}

func TestIgnoreSubdirectories(t *testing.T) {
	dir := t.TempDir()

	// Create directories that should be ignored
	os.MkdirAll(filepath.Join(dir, ".git", "objects"), 0755)
	os.MkdirAll(filepath.Join(dir, "node_modules", "pkg"), 0755)
	os.MkdirAll(filepath.Join(dir, "src"), 0755)

	w, err := New(dir)
	if err != nil {
		t.Fatalf("New failed: %v", err)
	}
	defer w.Stop()

	// Write to ignored dir
	os.WriteFile(filepath.Join(dir, ".git", "objects", "obj1"), []byte("data"), 0644)
	// Write to non-ignored dir
	os.WriteFile(filepath.Join(dir, "src", "main.go"), []byte("package main"), 0644)

	time.Sleep(200 * time.Millisecond)

	changes := w.RecentChanges(10)
	for _, c := range changes {
		if filepath.Base(c.Path) == "obj1" {
			t.Error("should not detect changes in .git directory")
		}
	}
}
