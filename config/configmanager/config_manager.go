package configmanager

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/bountyhub-org/runner/config"
)

type Manager struct {
	rootPath string
}

func New() (*Manager, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return nil, fmt.Errorf("failed to determine users home directory")
	}

	rootPath := filepath.Join(home, ".config", "bountyhub")
	if err := os.MkdirAll(rootPath, 0o700); err != nil {
		return nil, fmt.Errorf("failed to create config directory: %w", err)
	}

	return &Manager{rootPath: rootPath}, nil
}

func (m *Manager) Store(cfg config.Config) error {
	targetDir := filepath.Join(m.rootPath, cfg.Name)
	if err := os.MkdirAll(targetDir, 0o700); err != nil {
		return fmt.Errorf("failed to create config directory: %w", err)
	}

	targetPath := filepath.Join(targetDir, "config.yaml")

	f, err := os.OpenFile(targetPath, os.O_RDWR|os.O_CREATE|os.O_TRUNC, 0o600)
	if err != nil {
		return fmt.Errorf("failed to open config file for writing: %w", err)
	}
	defer f.Close()

	if err := json.NewEncoder(f).Encode(&cfg); err != nil {
		return fmt.Errorf("failed to encode config: %w", err)
	}

	return nil
}

func (m *Manager) Load(name string) (*config.Config, error) {
	targetPath := filepath.Join(m.rootPath, name, "config.yaml")

	f, err := os.Open(targetPath)
	if err != nil {
		return nil, fmt.Errorf("failed to open config file for reading: %w", err)
	}
	defer f.Close()

	var cfg config.Config
	if err := json.NewDecoder(f).Decode(&cfg); err != nil {
		return nil, fmt.Errorf("failed to decode config: %w", err)
	}
	if err := cfg.Validate(); err != nil {
		return nil, fmt.Errorf("invalid config: %w", err)
	}

	return &cfg, nil
}

func (m *Manager) Exists(name string) (bool, error) {
	targetPath := filepath.Join(m.rootPath, name, "config.yaml")
	fi, err := os.Stat(targetPath)
	switch {
	case errors.Is(err, os.ErrNotExist):
		return false, nil
	case err != nil:
		return false, fmt.Errorf("failed to check config %q existence: %w", targetPath, err)
	case fi.IsDir():
		return false, fmt.Errorf("config path %q is a directory, expected a file", targetPath)
	default:
		return true, nil
	}
}

func (m *Manager) Remove(name string) error {
	targetDir := filepath.Join(m.rootPath, name)
	if err := os.RemoveAll(targetDir); err != nil {
		return fmt.Errorf("failed to remove config: %w", err)
	}

	return nil
}
