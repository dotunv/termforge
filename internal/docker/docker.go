package docker

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"time"
)

type ContainerState string

const (
	StateRunning ContainerState = "running"
	StateExited  ContainerState = "exited"
	StatePaused  ContainerState = "paused"
	StateCreated ContainerState = "created"
)

type Container struct {
	ID      string         `json:"id"`
	Name    string         `json:"name"`
	Image   string         `json:"image"`
	State   ContainerState `json:"state"`
	Status  string         `json:"status"`
	Ports   string         `json:"ports"`
	Created time.Time      `json:"created"`
}

type DockerStatus struct {
	Available  bool        `json:"available"`
	Version    string      `json:"version"`
	Containers []Container `json:"containers"`
	Error      string      `json:"error,omitempty"`
}

type Service struct {
	timeout time.Duration
}

func NewService() *Service {
	return &Service{
		timeout: 10 * time.Second,
	}
}

func (s *Service) runCommand(name string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), s.timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, name, args...)
	var out bytes.Buffer
	cmd.Stdout = &out
	cmd.Stderr = &out
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("%s: %w", name, err)
	}
	return out.String(), nil
}

func (s *Service) IsAvailable() bool {
	out, err := s.runCommand("docker", "info", "--format", "{{.ServerVersion}}")
	if err != nil {
		return false
	}
	_ = out
	return true
}

func (s *Service) GetStatus() DockerStatus {
	out, err := s.runCommand("docker", "info", "--format", "{{.ServerVersion}}")
	if err != nil {
		return DockerStatus{
			Available: false,
			Error:     "Docker not available",
		}
	}

	containers, err := s.listContainers()
	if err != nil {
		return DockerStatus{
			Available: true,
			Version:   out,
			Error:     err.Error(),
		}
	}

	return DockerStatus{
		Available:  true,
		Version:    out,
		Containers: containers,
	}
}

func (s *Service) listContainers() ([]Container, error) {
	out, err := s.runCommand("docker", "ps", "-a", "--format", `{"id":"{{.ID}}","name":"{{.Names}}","image":"{{.Image}}","state":"{{.State}}","status":"{{.Status}}","ports":"{{.Ports}}","created":"{{.CreatedAt}}"}`)
	if err != nil {
		return nil, fmt.Errorf("list containers: %w", err)
	}

	var containers []Container
	decoder := json.NewDecoder(bytes.NewReader([]byte(out)))
	for decoder.More() {
		var c Container
		if err := decoder.Decode(&c); err != nil {
			continue
		}
		c.State = ContainerState(c.State)
		containers = append(containers, c)
	}

	return containers, nil
}

func (s *Service) StartContainer(id string) error {
	_, err := s.runCommand("docker", "start", id)
	return err
}

func (s *Service) StopContainer(id string) error {
	_, err := s.runCommand("docker", "stop", id)
	return err
}
