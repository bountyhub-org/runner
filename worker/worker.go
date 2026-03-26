package worker

import (
	"context"

	"github.com/bountyhub-org/runner/api/jobexecutionv1connect"
)

type Worker interface {
	Work(ctx context.Context, assignedJob *jobexecutionv1connect.AssignedJob) error
}
