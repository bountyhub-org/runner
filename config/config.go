package config

import (
	"errors"

	"github.com/google/uuid"
)

type Config struct {
	Token    uuid.UUID `json:"token"`
	Name     string    `json:"name"`
	Capacity uint32    `json:"capacity"`
}

func (c *Config) Validate() error {
	if c.Token.Version() != 4 {
		return errors.New("token must be a valid uuid v4")
	}
	if err := ValidateName(c.Name); err != nil {
		return err
	}
	if err := ValidateCapacity(c.Capacity); err != nil {
		return err
	}

	return nil
}

func ValidateToken(token string) error {
	parsed, err := uuid.Parse(token)
	if err != nil {
		return errors.New("token must be a valid uuid v4")
	}
	if parsed.Version() != 4 {
		return errors.New("token must be a valid uuid v4")
	}
	return nil
}

func ValidateName(name string) error {
	if name == "" {
		return errors.New("name must not be empty")
	}
	if len(name) > 255 {
		return errors.New("name must be less than 256 characters")
	}
	// TODO: add validation
	return nil
}

func ValidateCapacity(capacity uint32) error {
	if capacity == 0 {
		return errors.New("capacity must be greater than 0")
	}
	if capacity > 1024 {
		return errors.New("capacity must be less than or equal to 1024")
	}
	return nil
}
