package executor

import (
	"context"

	"github.com/bountyhub-org/runner/api/jobassignmentv1connect"
)

type Executor interface {
	Execute(ctx context.Context, job *jobassignmentv1connect.AcquireJobsRequest) error
}
