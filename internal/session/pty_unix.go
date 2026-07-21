//go:build !windows

package session

import (
	"os"
	"os/exec"

	"github.com/creack/pty"
)

func defaultShell() string {
	if sh := os.Getenv("SHELL"); sh != "" {
		return sh
	}
	return "bash"
}

func startPty(cwd string) (*os.File, error) {
	shell := defaultShell()
	cmd := exec.Command(shell)
	cmd.Dir = cwd
	return pty.Start(cmd)
}

func resizePty(s *Session, width, height int) error {
	f, ok := s.Pty.(*os.File)
	if !ok {
		return nil
	}
	return pty.Setsize(f, &pty.Winsize{Rows: uint16(height), Cols: uint16(width)})
}
