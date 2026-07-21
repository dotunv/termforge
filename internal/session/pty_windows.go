//go:build windows

package session

import (
	"fmt"
	"os"

	"github.com/UserExistsError/conpty"
)

func defaultShell() string {
	return "powershell.exe"
}

func startPty(cwd string) (*conpty.ConPty, error) {
	if cwd == "" {
		cwd, _ = os.Getwd()
	}

	width := 120
	height := 30

	cpty, err := conpty.Start(defaultShell(),
		conpty.ConPtyDimensions(width, height),
		conpty.ConPtyWorkDir(cwd),
	)
	if err != nil {
		return nil, fmt.Errorf("conpty start: %w", err)
	}

	return cpty, nil
}

func resizePty(s *Session, width, height int) error {
	cpty, ok := s.Pty.(*conpty.ConPty)
	if !ok {
		return nil
	}
	return cpty.Resize(width, height)
}
