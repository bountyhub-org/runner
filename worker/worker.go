package worker

import (
	"context"

	"github.com/bountyhub-org/runner/api/jobexecutionv1connect"
)

type Worker interface {
	Work(ctx context.Context, job jobexecutionv1connect.CreateExecutionResponse) error
}
