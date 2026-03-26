package configmanager

import "os"

type Config struct {
	Token    string `json:"token"`
	Name     string `json:"name"`
	Capacity uint32 `json:"capacity"`
}

type ManagerConfig struct {
	RootPath string
}

func (c *ManagerConfig) Validate() error {
	s, err := os.Stat(c.RootPath)
	if err != nil {
		return err
	}

	if !s.IsDir() {
		return os.ErrInvalid
	}
	return nil
}

func New(cfg ManagerConfig) (*Manager, error) {
	if err := cfg.Validate(); err != nil {
		return nil, err
	}
	return &Manager{
		rootPath: cfg.RootPath,
	}, nil
}

type Manager struct {
	rootPath string
}
