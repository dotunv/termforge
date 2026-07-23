package knowledge

import (
	"testing"

	"github.com/termforge/forge/internal/storage"
)

func setupTestDB(t *testing.T) *storage.DB {
	t.Helper()
	dir := t.TempDir()
	db, err := storage.NewSQLite(dir)
	if err != nil {
		t.Fatalf("failed to create test db: %v", err)
	}
	if err := db.RunMigrations(); err != nil {
		t.Fatalf("failed to run migrations: %v", err)
	}
	db.UpsertProject("p1", "Test Project", "/tmp")
	t.Cleanup(func() { db.Close() })
	return db
}

func TestCreateNode(t *testing.T) {
	db := setupTestDB(t)

	node, err := CreateNode(db, "p1", "Test Note", "Some content", NodeNote)
	if err != nil {
		t.Fatalf("CreateNode failed: %v", err)
	}
	if node.Title != "Test Note" {
		t.Fatalf("expected Test Note, got %s", node.Title)
	}
	if node.Content != "Some content" {
		t.Fatalf("expected Some content, got %s", node.Content)
	}
	if node.NodeType != NodeNote {
		t.Fatalf("expected note, got %s", node.NodeType)
	}
	if node.ProjectID != "p1" {
		t.Fatalf("expected p1, got %s", node.ProjectID)
	}
}

func TestGetNode(t *testing.T) {
	db := setupTestDB(t)

	created, _ := CreateNode(db, "p1", "Decision", "Important", NodeDecision)

	got, err := GetNode(db, created.ID)
	if err != nil {
		t.Fatalf("GetNode failed: %v", err)
	}
	if got == nil {
		t.Fatal("expected non-nil node")
	}
	if got.Title != "Decision" {
		t.Fatalf("expected Decision, got %s", got.Title)
	}
	if got.NodeType != NodeDecision {
		t.Fatalf("expected decision, got %s", got.NodeType)
	}
}

func TestGetNodeNotFound(t *testing.T) {
	db := setupTestDB(t)

	got, err := GetNode(db, "nonexistent")
	if err != nil {
		t.Fatalf("GetNode failed: %v", err)
	}
	if got != nil {
		t.Fatal("expected nil for nonexistent node")
	}
}

func TestListNodes(t *testing.T) {
	db := setupTestDB(t)

	CreateNode(db, "p1", "Note 1", "Content 1", NodeNote)
	CreateNode(db, "p1", "Note 2", "Content 2", NodeDecision)
	CreateNode(db, "p1", "Note 3", "Content 3", NodeArchitecture)

	nodes, err := ListNodes(db, "p1")
	if err != nil {
		t.Fatalf("ListNodes failed: %v", err)
	}
	if len(nodes) != 3 {
		t.Fatalf("expected 3 nodes, got %d", len(nodes))
	}
}

func TestUpdateNode(t *testing.T) {
	db := setupTestDB(t)

	created, _ := CreateNode(db, "p1", "Original", "Original content", NodeNote)

	err := UpdateNode(db, created.ID, "Updated", "Updated content")
	if err != nil {
		t.Fatalf("UpdateNode failed: %v", err)
	}

	got, _ := GetNode(db, created.ID)
	if got.Title != "Updated" {
		t.Fatalf("expected Updated, got %s", got.Title)
	}
	if got.Content != "Updated content" {
		t.Fatalf("expected Updated content, got %s", got.Content)
	}
}

func TestDeleteNode(t *testing.T) {
	db := setupTestDB(t)

	created, _ := CreateNode(db, "p1", "To Delete", "content", NodeNote)

	err := DeleteNode(db, created.ID)
	if err != nil {
		t.Fatalf("DeleteNode failed: %v", err)
	}

	got, _ := GetNode(db, created.ID)
	if got != nil {
		t.Fatal("expected nil after delete")
	}
}

func TestSearchNodes(t *testing.T) {
	db := setupTestDB(t)

	CreateNode(db, "p1", "Architecture Decision", "We chose Go", NodeDecision)
	CreateNode(db, "p1", "API Notes", "REST endpoints", NodeNote)
	CreateNode(db, "p1", "Database Schema", "PostgreSQL", NodeArchitecture)

	nodes, err := SearchNodes(db, "p1", "Architecture")
	if err != nil {
		t.Fatalf("SearchNodes failed: %v", err)
	}
	if len(nodes) != 1 {
		t.Fatalf("expected 1 node, got %d", len(nodes))
	}

	nodes, err = SearchNodes(db, "p1", "Go")
	if err != nil {
		t.Fatalf("SearchNodes failed: %v", err)
	}
	if len(nodes) != 1 {
		t.Fatalf("expected 1 node, got %d", len(nodes))
	}
}

func TestNodeTypes(t *testing.T) {
	db := setupTestDB(t)

	types := []struct {
		nodeType NodeType
		expected string
	}{
		{NodeDecision, "decision"},
		{NodeNote, "note"},
		{NodeArchitecture, "architecture_record"},
	}

	for _, tc := range types {
		node, err := CreateNode(db, "p1", string(tc.nodeType), "content", tc.nodeType)
		if err != nil {
			t.Fatalf("CreateNode failed for %s: %v", tc.nodeType, err)
		}
		if string(node.NodeType) != tc.expected {
			t.Fatalf("expected %s, got %s", tc.expected, node.NodeType)
		}
	}
}
