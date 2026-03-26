package localworker

import (
	"context"
	"fmt"
	"log/slog"

	"connectrpc.com/connect"
	"github.com/bountyhub-org/runner/api/jobexecutionv1connect"
	"github.com/bountyhub-org/runner/expr"
	"github.com/bountyhub-org/runner/worker"
)

var _ worker.Worker = (*LocalWorker)(nil)

type Client interface {
	ResolveJob(context.Context, *connect.Request[jobexecutionv1connect.ResolveJobRequest]) (*connect.Response[jobexecutionv1connect.ResolveJobResponse], error)
}

type Config struct {
	Client Client
	Logger *slog.Logger
}

func (c *Config) Validate() error {
	if c.Client == nil {
		return fmt.Errorf("client is required")
	}
	if c.Logger == nil {
		return fmt.Errorf("logger is required")
	}
	return nil
}

func New(cfg Config) (*LocalWorker, error) {
	return &LocalWorker{
		client: cfg.Client,
	}, nil
}

type LocalWorker struct {
	client Client
	logger *slog.Logger
}

// Work implements [worker.Worker].
func (c *LocalWorker) Work(ctx context.Context, assignedJob *jobexecutionv1connect.AssignedJob) error {
	log := c.logger.With("job_id", assignedJob.Id)

	log.Info("resolving a job")
	res, err := c.client.ResolveJob(ctx, connect.NewRequest(&jobexecutionv1connect.ResolveJobRequest{
		Id:      assignedJob.Id,
		Session: assignedJob.Session,
	}))
	if err != nil {
		return fmt.Errorf("failed to resolve the job: %w", err)
	}

	exprEngine := expr.NewEngine(res.Msg)

	return nil
}
