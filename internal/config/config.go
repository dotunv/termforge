package config

import (
	"encoding/json"
	"os"
	"path/filepath"
)

type Config struct {
	ListenAddr string `json:"listen_addr"`
	DataDir    string `json:"data_dir"`
	Shell      string `json:"shell"`
}

func Load(path string, dataDir string) (*Config, error) {
	cfg := &Config{
		ListenAddr: "127.0.0.1:9400",
		Shell:      "bash",
	}

	if path != "" {
		f, err := os.Open(path)
		if err != nil {
			return nil, err
		}
		defer f.Close()
		if err := json.NewDecoder(f).Decode(cfg); err != nil {
			return nil, err
		}
	}

	if dataDir != "" {
		cfg.DataDir = dataDir
	}

	if cfg.DataDir == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return nil, err
		}
		cfg.DataDir = filepath.Join(home, ".forge")
	}

	if err := os.MkdirAll(cfg.DataDir, 0o755); err != nil {
		return nil, err
	}

	return cfg, nil
}
