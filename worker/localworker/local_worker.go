package localworker

import (
	"context"
	"fmt"
	"os"

	"connectrpc.com/connect"
	"github.com/bountyhub-org/runner/api/jobexecutionv1connect"
	"github.com/bountyhub-org/runner/expr"
	"github.com/bountyhub-org/runner/joblogger"
	"github.com/bountyhub-org/runner/step"
	"github.com/bountyhub-org/runner/worker"
)

var _ worker.Worker = (*LocalWorker)(nil)

type Client interface {
	ResolveJob(context.Context, *connect.Request[jobexecutionv1connect.ResolveJobRequest]) (*connect.Response[jobexecutionv1connect.ResolveJobResponse], error)
}

type JobLogger interface {
	Stdout(ctx context.Context, text string)
	Stderr(ctx context.Context, text string)
}

type Config struct {
	Client Client
	Logger JobLogger
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
		logger: cfg.Logger,
	}, nil
}

type LocalWorker struct {
	client Client
	logger JobLogger
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
	_ = exprEngine

	return nil
}

type setupStep struct {
	baseDir string
	step    *jobexecutionv1connect.Step
	logger  *joblogger.JobLogger
}

func (s *setupStep) run(ctx context.Context) (step.Result, error) {
	s.logger.Stderr(fmt.Sprintf("creating the base directory: %s", s.baseDir))
	if err := os.MkdirAll(s.baseDir, 0o755); err != nil {
		return step.ResultFailed(), fmt.Errorf("failed to create the base directory: %w", err)
	}
	return step.ResultSucceeded(step.StatusSucceeded), nil
}
