package knowledge

import (
	"database/sql"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/termforge/forge/internal/storage"
)

type NodeType string

const (
	NodeDecision        NodeType = "decision"
	NodeNote            NodeType = "note"
	NodeArchitecture    NodeType = "architecture_record"
)

type KnowledgeNode struct {
	ID        string
	ProjectID string
	Title     string
	Content   string
	NodeType  NodeType
	CreatedAt time.Time
}

func CreateNode(db *storage.DB, projectID, title, content string, nodeType NodeType) (*KnowledgeNode, error) {
	id := uuid.New().String()
	_, err := db.Conn().Exec(
		`INSERT INTO knowledge_nodes (id, project_id, title, content, node_type, created_at)
		 VALUES (?, ?, ?, ?, ?, ?)`,
		id, projectID, title, content, string(nodeType), time.Now(),
	)
	if err != nil {
		return nil, fmt.Errorf("create knowledge node: %w", err)
	}
	return &KnowledgeNode{
		ID:        id,
		ProjectID: projectID,
		Title:     title,
		Content:   content,
		NodeType:  nodeType,
		CreatedAt: time.Now(),
	}, nil
}

func GetNode(db *storage.DB, id string) (*KnowledgeNode, error) {
	var n KnowledgeNode
	var nodeType string
	err := db.Conn().QueryRow(
		`SELECT id, project_id, title, content, node_type, created_at
		 FROM knowledge_nodes WHERE id = ?`, id,
	).Scan(&n.ID, &n.ProjectID, &n.Title, &n.Content, &nodeType, &n.CreatedAt)
	if err == sql.ErrNoRows {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("get knowledge node: %w", err)
	}
	n.NodeType = NodeType(nodeType)
	return &n, nil
}

func ListNodes(db *storage.DB, projectID string) ([]KnowledgeNode, error) {
	rows, err := db.Conn().Query(
		`SELECT id, project_id, title, content, node_type, created_at
		 FROM knowledge_nodes WHERE project_id = ?
		 ORDER BY created_at DESC`, projectID,
	)
	if err != nil {
		return nil, fmt.Errorf("list knowledge nodes: %w", err)
	}
	defer rows.Close()

	var nodes []KnowledgeNode
	for rows.Next() {
		var n KnowledgeNode
		var nodeType string
		if err := rows.Scan(&n.ID, &n.ProjectID, &n.Title, &n.Content, &nodeType, &n.CreatedAt); err != nil {
			return nil, fmt.Errorf("scan knowledge node: %w", err)
		}
		n.NodeType = NodeType(nodeType)
		nodes = append(nodes, n)
	}
	return nodes, nil
}

func UpdateNode(db *storage.DB, id, title, content string) error {
	_, err := db.Conn().Exec(
		`UPDATE knowledge_nodes SET title = ?, content = ? WHERE id = ?`,
		title, content, id,
	)
	if err != nil {
		return fmt.Errorf("update knowledge node: %w", err)
	}
	return nil
}

func DeleteNode(db *storage.DB, id string) error {
	_, err := db.Conn().Exec(`DELETE FROM knowledge_nodes WHERE id = ?`, id)
	if err != nil {
		return fmt.Errorf("delete knowledge node: %w", err)
	}
	return nil
}

func SearchNodes(db *storage.DB, projectID, query string) ([]KnowledgeNode, error) {
	rows, err := db.Conn().Query(
		`SELECT id, project_id, title, content, node_type, created_at
		 FROM knowledge_nodes
		 WHERE project_id = ? AND (title LIKE ? OR content LIKE ?)
		 ORDER BY created_at DESC`, projectID, "%"+query+"%", "%"+query+"%",
	)
	if err != nil {
		return nil, fmt.Errorf("search knowledge nodes: %w", err)
	}
	defer rows.Close()

	var nodes []KnowledgeNode
	for rows.Next() {
		var n KnowledgeNode
		var nodeType string
		if err := rows.Scan(&n.ID, &n.ProjectID, &n.Title, &n.Content, &nodeType, &n.CreatedAt); err != nil {
			return nil, fmt.Errorf("scan knowledge node: %w", err)
		}
		n.NodeType = NodeType(nodeType)
		nodes = append(nodes, n)
	}
	return nodes, nil
}
