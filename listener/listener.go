package listener

import (
	"context"

	"github.com/bountyhub-org/runner/worker"
)

type Listener struct{}

func (l *Listener) Run(ctx context.Context, w worker.Worker) error {
	panic("todo")
}
